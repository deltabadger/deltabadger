//! Rails' I18n as this app uses it: config/locales/*.yml (embedded by build.rs), looked up by full
//! key with a fallback to English, `%{name}` interpolation, and pluralisation with Rails' default
//! one/other rule plus the Russian rule in config/locales/plurals.rb.
//! `t` is the view helper (ActionView's `translate`); `text` is `I18n.t`, which controllers use.

include!(concat!(env!("OUT_DIR"), "/translations.rs"));

pub const DEFAULT: &str = "en";

/// Keys Rails takes from a gem's own locale file, not from config/locales. Pinned by tests/i18n.rs:
/// Devise's lock message, and the dotiw gem's unit names (`distance_of_time_in_words`) in the ten
/// of this app's locales the gem has a file for.
const FROM_GEMS: &[(&str, &str)] = &[
    ("en.devise.failure.locked", "Your account is locked."),
    ("en.errors.messages.taken", "has already been taken"),
    ("da.datetime.dotiw.days.one", "1 dag"), ("da.datetime.dotiw.days.other", "%{count} dage"), ("da.datetime.dotiw.hours.one", "1 time"),
    ("da.datetime.dotiw.hours.other", "%{count} timer"), ("da.datetime.dotiw.less_than_x", "mindre end %{distance}"),
    ("da.datetime.dotiw.minutes.one", "1 minut"), ("da.datetime.dotiw.minutes.other", "%{count} minutter"), ("da.datetime.dotiw.months.one", "1 måned"),
    ("da.datetime.dotiw.months.other", "%{count} måneder"), ("da.datetime.dotiw.seconds.one", "1 sekund"),
    ("da.datetime.dotiw.seconds.other", "%{count} sekunder"), ("da.datetime.dotiw.weeks.one", "1 uge"), ("da.datetime.dotiw.weeks.other", "%{count} uger"),
    ("da.datetime.dotiw.years", "%{count} år"), ("de.datetime.dotiw.days.one", "1 Tag"), ("de.datetime.dotiw.days.other", "%{count} Tage"),
    ("de.datetime.dotiw.hours.one", "1 Stunde"), ("de.datetime.dotiw.hours.other", "%{count} Stunden"),
    ("de.datetime.dotiw.less_than_x", "weniger als %{distance}"), ("de.datetime.dotiw.minutes.one", "1 Minute"),
    ("de.datetime.dotiw.minutes.other", "%{count} Minuten"), ("de.datetime.dotiw.months.one", "1 Monat"),
    ("de.datetime.dotiw.months.other", "%{count} Monate"), ("de.datetime.dotiw.seconds.one", "1 Sekunde"),
    ("de.datetime.dotiw.seconds.other", "%{count} Sekunden"), ("de.datetime.dotiw.weeks.one", "1 Woche"),
    ("de.datetime.dotiw.weeks.other", "%{count} Wochen"), ("de.datetime.dotiw.years.one", "1 Jahr"), ("de.datetime.dotiw.years.other", "%{count} Jahre"),
    ("en.datetime.dotiw.days.one", "1 day"), ("en.datetime.dotiw.days.other", "%{count} days"), ("en.datetime.dotiw.hours.one", "1 hour"),
    ("en.datetime.dotiw.hours.other", "%{count} hours"), ("en.datetime.dotiw.less_than_x", "less than %{distance}"),
    ("en.datetime.dotiw.minutes.one", "1 minute"), ("en.datetime.dotiw.minutes.other", "%{count} minutes"), ("en.datetime.dotiw.months.one", "1 month"),
    ("en.datetime.dotiw.months.other", "%{count} months"), ("en.datetime.dotiw.seconds.one", "1 second"),
    ("en.datetime.dotiw.seconds.other", "%{count} seconds"), ("en.datetime.dotiw.weeks.one", "1 week"), ("en.datetime.dotiw.weeks.other", "%{count} weeks"),
    ("en.datetime.dotiw.years.one", "1 year"), ("en.datetime.dotiw.years.other", "%{count} years"), ("es.datetime.dotiw.days.one", "un día"),
    ("es.datetime.dotiw.days.other", "%{count} días"), ("es.datetime.dotiw.hours.one", "una hora"), ("es.datetime.dotiw.hours.other", "%{count} horas"),
    ("es.datetime.dotiw.less_than_x", "menos de %{distance}"), ("es.datetime.dotiw.minutes.one", "un minuto"),
    ("es.datetime.dotiw.minutes.other", "%{count} minutos"), ("es.datetime.dotiw.months.one", "un mes"), ("es.datetime.dotiw.months.other", "%{count} meses"),
    ("es.datetime.dotiw.seconds.one", "un segundo"), ("es.datetime.dotiw.seconds.other", "%{count} segundos"), ("es.datetime.dotiw.weeks.one", "una semana"),
    ("es.datetime.dotiw.weeks.other", "%{count} semanas"), ("es.datetime.dotiw.years.one", "un año"), ("es.datetime.dotiw.years.other", "%{count} años"),
    ("fr.datetime.dotiw.days.one", "1 jour"), ("fr.datetime.dotiw.days.other", "%{count} jours"), ("fr.datetime.dotiw.hours.one", "1 heure"),
    ("fr.datetime.dotiw.hours.other", "%{count} heures"), ("fr.datetime.dotiw.less_than_x", "moins de %{distance}"),
    ("fr.datetime.dotiw.minutes.one", "1 minute"), ("fr.datetime.dotiw.minutes.other", "%{count} minutes"), ("fr.datetime.dotiw.months.one", "1 mois"),
    ("fr.datetime.dotiw.months.other", "%{count} mois"), ("fr.datetime.dotiw.seconds.one", "1 seconde"),
    ("fr.datetime.dotiw.seconds.other", "%{count} secondes"), ("fr.datetime.dotiw.weeks.one", "1 semaine"),
    ("fr.datetime.dotiw.weeks.other", "%{count} semaines"), ("fr.datetime.dotiw.years.one", "1 an"), ("fr.datetime.dotiw.years.other", "%{count} ans"),
    ("it.datetime.dotiw.days.one", "un giorno"), ("it.datetime.dotiw.days.other", "%{count} giorni"), ("it.datetime.dotiw.hours.one", "una ora"),
    ("it.datetime.dotiw.hours.other", "%{count} ore"), ("it.datetime.dotiw.minutes.one", "un minuto"), ("it.datetime.dotiw.minutes.other", "%{count} minuti"),
    ("it.datetime.dotiw.months.one", "un mese"), ("it.datetime.dotiw.months.other", "%{count} mesi"), ("it.datetime.dotiw.seconds.one", "un secondo"),
    ("it.datetime.dotiw.seconds.other", "%{count} secondi"), ("it.datetime.dotiw.weeks.one", "una settimana"),
    ("it.datetime.dotiw.weeks.other", "%{count} settimane"), ("it.datetime.dotiw.years.one", "un anno"), ("it.datetime.dotiw.years.other", "%{count} anni"),
    ("nl.datetime.dotiw.days.one", "1 dag"), ("nl.datetime.dotiw.days.other", "%{count} dagen"), ("nl.datetime.dotiw.hours.one", "1 uur"),
    ("nl.datetime.dotiw.hours.other", "%{count} uur"), ("nl.datetime.dotiw.less_than_x", "minder dan %{distance}"),
    ("nl.datetime.dotiw.minutes.one", "1 minuut"), ("nl.datetime.dotiw.minutes.other", "%{count} minuten"), ("nl.datetime.dotiw.months.one", "1 maand"),
    ("nl.datetime.dotiw.months.other", "%{count} maanden"), ("nl.datetime.dotiw.seconds.one", "1 seconde"),
    ("nl.datetime.dotiw.seconds.other", "%{count} seconden"), ("nl.datetime.dotiw.weeks.one", "1 week"), ("nl.datetime.dotiw.weeks.other", "%{count} weken"),
    ("nl.datetime.dotiw.years.one", "1 jaar"), ("nl.datetime.dotiw.years.other", "%{count} jaar"), ("pl.datetime.dotiw.days.few", "%{count} dni"),
    ("pl.datetime.dotiw.days.many", "%{count} dni"), ("pl.datetime.dotiw.days.one", "1 dzień"), ("pl.datetime.dotiw.days.other", "%{count} dni"),
    ("pl.datetime.dotiw.hours.few", "%{count} godziny"), ("pl.datetime.dotiw.hours.many", "%{count} godzin"), ("pl.datetime.dotiw.hours.one", "1 godzina"),
    ("pl.datetime.dotiw.hours.other", "%{count} godzin"), ("pl.datetime.dotiw.less_than_x", "mniej niż %{distance}"),
    ("pl.datetime.dotiw.minutes.few", "%{count} minuty"), ("pl.datetime.dotiw.minutes.many", "%{count} minut"), ("pl.datetime.dotiw.minutes.one", "1 minuta"),
    ("pl.datetime.dotiw.minutes.other", "%{count} minut"), ("pl.datetime.dotiw.months.few", "%{count} miesiące"),
    ("pl.datetime.dotiw.months.many", "%{count} miesięcy"), ("pl.datetime.dotiw.months.one", "1 miesiąc"),
    ("pl.datetime.dotiw.months.other", "%{count} miesięcy"), ("pl.datetime.dotiw.seconds.few", "%{count} sekundy"),
    ("pl.datetime.dotiw.seconds.many", "%{count} sekund"), ("pl.datetime.dotiw.seconds.one", "1 sekunda"),
    ("pl.datetime.dotiw.seconds.other", "%{count} sekund"), ("pl.datetime.dotiw.weeks.few", "%{count} tygodnie"),
    ("pl.datetime.dotiw.weeks.many", "%{count} tygodni"), ("pl.datetime.dotiw.weeks.one", "1 tydzień"), ("pl.datetime.dotiw.weeks.other", "%{count} tygodni"),
    ("pl.datetime.dotiw.years.few", "%{count} lata"), ("pl.datetime.dotiw.years.many", "%{count} lat"), ("pl.datetime.dotiw.years.one", "1 rok"),
    ("pl.datetime.dotiw.years.other", "%{count} lat"), ("ru.datetime.dotiw.days.few", "%{count} дня"), ("ru.datetime.dotiw.days.many", "%{count} дней"),
    ("ru.datetime.dotiw.days.one", "%{count} день"), ("ru.datetime.dotiw.days.other", "%{count} дня"), ("ru.datetime.dotiw.hours.few", "%{count} часа"),
    ("ru.datetime.dotiw.hours.many", "%{count} часов"), ("ru.datetime.dotiw.hours.one", "%{count} час"), ("ru.datetime.dotiw.hours.other", "%{count} часа"),
    ("ru.datetime.dotiw.less_than_x", "меньше, чем %{distance}"), ("ru.datetime.dotiw.minutes.few", "%{count} минуты"),
    ("ru.datetime.dotiw.minutes.many", "%{count} минут"), ("ru.datetime.dotiw.minutes.one", "%{count} минута"),
    ("ru.datetime.dotiw.minutes.other", "%{count} минуты"), ("ru.datetime.dotiw.months.few", "%{count} месяца"),
    ("ru.datetime.dotiw.months.many", "%{count} месяцев"), ("ru.datetime.dotiw.months.one", "%{count} месяц"),
    ("ru.datetime.dotiw.months.other", "%{count} месяца"), ("ru.datetime.dotiw.seconds.few", "%{count} секунды"),
    ("ru.datetime.dotiw.seconds.many", "%{count} секунд"), ("ru.datetime.dotiw.seconds.one", "%{count} секунда"),
    ("ru.datetime.dotiw.seconds.other", "%{count} секунды"), ("ru.datetime.dotiw.weeks.few", "%{count} недели"),
    ("ru.datetime.dotiw.weeks.many", "%{count} недель"), ("ru.datetime.dotiw.weeks.one", "%{count} неделя"),
    ("ru.datetime.dotiw.weeks.other", "%{count} недели"), ("ru.datetime.dotiw.years.few", "%{count} года"), ("ru.datetime.dotiw.years.many", "%{count} лет"),
    ("ru.datetime.dotiw.years.one", "%{count} год"), ("ru.datetime.dotiw.years.other", "%{count} года"), ("sv.datetime.dotiw.days.one", "1 dag"),
    ("sv.datetime.dotiw.days.other", "%{count} dagar"), ("sv.datetime.dotiw.hours.one", "1 timme"), ("sv.datetime.dotiw.hours.other", "%{count} timmar"),
    ("sv.datetime.dotiw.less_than_x", "mindre än %{distance}"), ("sv.datetime.dotiw.minutes.one", "1 minut"),
    ("sv.datetime.dotiw.minutes.other", "%{count} minuter"), ("sv.datetime.dotiw.months.one", "1 månad"),
    ("sv.datetime.dotiw.months.other", "%{count} månader"), ("sv.datetime.dotiw.seconds.one", "1 sekund"),
    ("sv.datetime.dotiw.seconds.other", "%{count} sekunder"), ("sv.datetime.dotiw.weeks.one", "1 vecka"),
    ("sv.datetime.dotiw.weeks.other", "%{count} veckor"), ("sv.datetime.dotiw.years.one", "1 år"), ("sv.datetime.dotiw.years.other", "%{count} år"),
];

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
