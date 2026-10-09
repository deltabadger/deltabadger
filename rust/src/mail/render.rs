//! The mailers' views: app/views/layouts/mailers/transactional.html.erb around app/views/bot_alerts_mailer/*,
//! test_mailer/* and devise/mailer/*, written out from what Rails renders (rust/tests/mail_parity.rs compares every byte).
//! Bot mails and the test mail are in the user's language (ApplicationMailer#set_locale); account mails are in the
//! language of the request that asked for them (Active Job carries I18n.locale), so those take the locale as an argument.
use super::Message;
use crate::engine::notice::{bounded, Notice, ERROR_LIMIT};
use crate::engine::{venue_rules, EngineError};
use crate::ruby::float_to_s;
use crate::web::i18n::{self, escape, Arg};
use crate::web::locale;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// `root_url`: where links and the logo point. config/environments/production.rb builds it from APP_ROOT_URL and FORCE_SSL.
pub struct Urls { pub root: String }

impl Urls {
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let app_root_url = env("APP_ROOT_URL").unwrap_or_else(|| "http://localhost:3000".into());
        let https = app_root_url.starts_with("https://");
        // Deltabadger::Application.force_ssl_from_env: FORCE_SSL when it is a recognised spelling, else the URL's scheme.
        let ssl = crate::web::env_boolean(env("FORCE_SSL")).unwrap_or(https);
        // `gsub(/^https?:\/\//, '').gsub(/\/.*$/, '')`: case-sensitive, so `HTTPS://host` keeps its scheme as the host.
        let rest = app_root_url.strip_prefix("https://").or_else(|| app_root_url.strip_prefix("http://")).unwrap_or(&app_root_url);
        let host = rest.split('/').next().unwrap_or(rest);
        Self { root: format!("{}://{host}/", if https || ssl { "https" } else { "http" }) }
    }
    /// A route helper's URL under ApplicationMailer#default_url_options: the locale prefix for every locale but the default.
    fn url(&self, locale: &str, path: &str) -> String { format!("{}{}", self.root.trim_end_matches('/'), locale::path(locale, path)) }
}

/// Whom a mail greets and where it goes.
pub struct Recipient { pub email: String, pub name: String, pub locale: &'static str }

/// `users.locale` as ApplicationMailer#set_locale reads it: NULL, or a value Rails does not know, is the default.
pub fn user_locale(stored: Option<&str>) -> &'static str { stored.and_then(locale::known).unwrap_or(locale::DEFAULT) }

fn layout(urls: &Urls, body: &str) -> String {
    format!(r#"<!DOCTYPE html>
<html>
  <head>
    <meta http-equiv="Content-Type" content="text/html; charset=utf-8">
    <style>
      body {{
        font-family: sans-serif;
      }}
      code {{
        font-size: 1.15em;
        margin: 2em 0;
        display: block;
      }}
      h1 {{
        font-size: 1.4em;
        margin: 1em 0 1.5em 0;
        font-family: Arial, sans-serif;
      }}
    </style>
  </head>
  <body>
    <div style="max-width: 700px; margin: 30px auto;">
      <img src="{root}logo_email_app.png" style="width:64px; height: 64px; margin: 1em 0;">

      {body}

      <p style="margin: 5em 0 1em 0;"></p>
      </p>
    </div>
  </body>
</html>
"#, root = escape(&urls.root))
}

/// `<h1>` with the subject, a blank line, the greeting: how every view of these mailers begins.
fn opening(locale: &str, subject: &str, name: &str) -> String {
    format!("<h1>{}</h1>\n\n<p>{}</p>\n", escape(subject), i18n::t(locale, "mailer.greeting_name", &[("name", Arg::Text(name))]))
}

fn message(from: &str, reply_to: bool, to: &str, subject: String, urls: &Urls, body: &str) -> Message {
    Message { from: from.to_string(), reply_to: reply_to.then(|| from.to_string()), to: to.to_string(), subject, html: layout(urls, body) }
}

/// BotAlertsMailer#end_of_funds. `exchange_name` as stored, `quote` the quote asset's symbol.
pub fn end_of_funds(from: &str, urls: &Urls, to: &Recipient, exchange_name: &str, quote: &str) -> Message {
    let args = [("exchange_name", Arg::Text(exchange_name)), ("quote", Arg::Text(quote))];
    let subject = i18n::text(to.locale, "bot_alerts_mailer.end_of_funds.subject", &args);
    let body = format!("{}{}\n", opening(to.locale, &subject, &to.name), i18n::t(to.locale, "bot_alerts_mailer.end_of_funds.template_html", &args));
    message(from, false, &to.email, subject, urls, &body)
}

/// BotAlertsMailer#notify_about_error (`stopped` false) and #stopped_by_error (`stopped` true). `error` is the sentence
/// Bot::ActionJob.humanized_errors hands the mailer; the venue's name is upper-cased by the mailer.
pub fn failure(from: &str, urls: &Urls, to: &Recipient, stopped: bool, label: &str, exchange_name: &str, error: &str) -> Message {
    let mail = if stopped { "stopped_by_error" } else { "notify_about_error" };
    let subject = i18n::text(to.locale, &format!("bot_alerts_mailer.{mail}.subject"), &[("label", Arg::Text(label))]);
    let upcased = exchange_name.to_uppercase();
    let html = i18n::t(to.locale, &format!("bot_alerts_mailer.{mail}.template_html"),
                       &[("label", Arg::Text(label)), ("exchange_name", Arg::Text(&upcased)), ("errors", Arg::Text(error))]);
    // stopped_by_error.html.erb has no line feed at its end; notify_about_error.html.erb has one.
    let body = format!("{}{html}{}", opening(to.locale, &subject, &to.name), if stopped { "" } else { "\n" });
    message(from, false, &to.email, subject, urls, &body)
}

/// BotAlertsMailer#stopped_by_amount_limit. `amount` is `quote_amount_limit.to_s`.
pub fn stopped_by_amount_limit(from: &str, urls: &Urls, to: &Recipient, label: &str, amount: &str, quote: &str) -> Message {
    let subject = i18n::text(to.locale, "bot_alerts_mailer.stopped_by_amount_limit.subject", &[("label", Arg::Text(label))]);
    let html = i18n::t(to.locale, "bot_alerts_mailer.stopped_by_amount_limit.template_html",
                       &[("label", Arg::Text(label)), ("amount", Arg::Text(amount)), ("quote", Arg::Text(quote))]);
    let body = format!("{}{html}", opening(to.locale, &subject, &to.name));
    message(from, false, &to.email, subject, urls, &body)
}

/// TestMailer#test_email (Settings: send a test email).
pub fn test_email(from: &str, urls: &Urls, to: &Recipient) -> Message {
    let subject = i18n::text(to.locale, "test_mailer.test_email.subject", &[]);
    let body = format!("{}<p>{}</p>\n", opening(to.locale, &subject, &to.name), i18n::t(to.locale, "test_mailer.test_email.body", &[]));
    message(from, false, &to.email, subject, urls, &body)
}

fn link(url: &str, text: &str) -> String { format!("<p>\n  <a href=\"{}\">{text}</a>\n</p>\n", escape(url)) }

/// Devise's reset_password_instructions through CustomDeviseMailer. `token` is the raw token, as Devise mails it.
pub fn reset_password_instructions(from: &str, urls: &Urls, to: &Recipient, token: &str) -> Message {
    let key = |name: &str| format!("devise.mailer.reset_password_instructions.{name}");
    let subject = i18n::text(to.locale, &key("subject"), &[]);
    let query: String = form_urlencoded::Serializer::new(String::new()).append_pair("reset_password_token", token).finish();
    let body = format!("{}{}\n\n{}\n{}\n{}\n", opening(to.locale, &subject, &to.name), i18n::t(to.locale, &key("template_html"), &[]),
                       link(&urls.url(to.locale, &format!("/password/edit?{query}")), &i18n::t(to.locale, &key("change_password_button"), &[])),
                       i18n::t(to.locale, &key("ignore_mail_html"), &[]), i18n::t(to.locale, &key("wont_change_html"), &[]));
    message(from, true, &to.email, subject, urls, &body)
}

/// Devise's confirmation_instructions. `to.email` is where it goes: the account's address, or the unconfirmed one when
/// the user is changing address (Devise passes `to:`).
pub fn confirmation_instructions(from: &str, urls: &Urls, to: &Recipient, token: &str) -> Message {
    let key = |name: &str| format!("devise.mailer.confirmation_instructions.{name}");
    let subject = i18n::text(to.locale, &key("subject"), &[]);
    let query: String = form_urlencoded::Serializer::new(String::new()).append_pair("confirmation_token", token).finish();
    let body = format!("{}{}\n\n{}", opening(to.locale, &subject, &to.name), i18n::t(to.locale, &key("template_html"), &[]),
                       link(&urls.url(to.locale, &format!("/confirmation?{query}")), &i18n::t(to.locale, &key("confirm_button"), &[])));
    message(from, true, &to.email, subject, urls, &body)
}

/// CustomDeviseMailer#email_already_taken: sent instead of an error when someone signs up with a registered address.
pub fn email_already_taken(from: &str, urls: &Urls, to: &Recipient) -> Message {
    let key = |name: &str| format!("devise.mailer.email_already_taken.{name}");
    let subject = i18n::text(to.locale, &key("subject"), &[]);
    // `new_password_url(@resource, locale: I18n.locale)`: the locale is always in the path here, the default too.
    let url = format!("{}{}/password/new", urls.root, to.locale);
    let body = format!("{}<p>{}</p>\n<p>{}</p>\n\n{}", opening(to.locale, &subject, &to.name), i18n::t(to.locale, &key("message"), &[]),
                       i18n::t(to.locale, &key("reset_password_message"), &[]), link(&url, &i18n::t(to.locale, &key("reset_password_button"), &[])));
    message(from, false, &to.email, subject, urls, &body)
}

/// Honeymaker::Exchanges::Kraken::ERROR_PATTERNS (0.12.3), first match wins: the code and what it names.
fn classify_kraken(message: &str) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    // /\AEAccount:Invalid permissions:(?<asset>\S+) trading restricted for (?<country>\w+)\.?\z/
    if let Some((asset, country)) = message.strip_prefix("EAccount:Invalid permissions:").and_then(|rest| rest.split_once(" trading restricted for ")) {
        let country = country.strip_suffix('.').unwrap_or(country);
        let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !asset.is_empty() && !asset.contains(char::is_whitespace) && word(country) {
            return Some(("regional_restriction", vec![("asset", asset.to_string()), ("country", country.to_string())]));
        }
    }
    if message.contains("EAPI:Invalid nonce") { return Some(("transient_nonce", vec![])); }
    if ["EGeneral:Internal error", "EService:Unavailable", "EService:Busy", "EService:Deadline elapsed"].iter().any(|p| message.contains(p)) {
        return Some(("transient_unavailable", vec![]));
    }
    None
}

/// Exchange#humanize_error in `locale`: the venue's own classifier (only Kraken has patterns), then the generic sentence
/// for the failure kind (Exchange::KIND_ERROR_KEYS), then the raw message.
pub fn humanize_error(exchange_type: &str, exchange_name: &str, locale: &str, message: &str) -> String {
    let translate = |code: &str, named: &[(&'static str, String)]| {
        let mut args: Vec<(&str, Arg)> = named.iter().map(|(k, v)| (*k, Arg::Text(v.as_str()))).collect();
        args.push(("exchange", Arg::Text(exchange_name)));
        i18n::text(locale, &format!("errors.exchange.{code}"), &args)
    };
    if exchange_type == "Exchanges::Kraken" {
        if let Some((code, named)) = classify_kraken(message) { return translate(code, &named); }
    }
    let kind = venue_rules::for_exchange(exchange_type).and_then(|rules| rules.failure_kind(std::slice::from_ref(&message.to_string())));
    match kind {
        Some("throttle") => translate("rate_limited", &[]),
        Some("transient") => translate("transient_unavailable", &[]),
        Some(kind) => translate(kind, &[]),
        None => message.to_string(),
    }
}

/// The most a label or a name may be in a mail; longer ones are cut, with an ellipsis (a listed divergence: Rails has
/// no bound). Errors are bounded when their marker is written (`notice::ERROR_LIMIT`) and again here.
pub const NAME_LIMIT: usize = 200;

/// A settings value as Ruby prints it in a translation: `quote_amount_limit.to_s`.
fn ruby_to_s(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n.as_i64().map(|i| i.to_string()).or_else(|| n.as_f64().map(float_to_s)).unwrap_or_default(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// The mail a marker on bot `bot_id` asks for, from the rows as they are now. None when Rails would send nothing: the
/// bot's user, its venue or (where the mail names it) its quote asset is gone.
/// A bot whose label is blank is named by Rails the first time it loads the row; here the label is used as stored
/// (a listed divergence that needs a bot Rails never loaded).
pub fn for_notice(c: &Connection, bot_id: i64, notice: &Notice, from: &str, urls: &Urls) -> Result<Option<Message>, EngineError> {
    type Row = (Option<String>, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
    let row: Option<Row> = c.query_row(
        "SELECT b.label, b.settings, u.email, u.name, u.locale, e.name, e.type FROM bots b JOIN users u ON u.id = b.user_id \
         LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1",
        [bot_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).optional()?;
    let Some((label, settings, Some(email), name, stored_locale, exchange_name, exchange_type)) = row else { return Ok(None) };
    let settings: Value = serde_json::from_str(&settings).map_err(|e| EngineError::Data(format!("bot {bot_id}: settings: {e}")))?;
    let to = Recipient { email, name: bounded(&name.unwrap_or_default(), NAME_LIMIT), locale: user_locale(stored_locale.as_deref()) };
    let label = bounded(&label.unwrap_or_default(), NAME_LIMIT);
    let symbol = |asset_id: Option<i64>| -> Result<Option<String>, EngineError> {
        let Some(id) = asset_id else { return Ok(None) };
        Ok(c.query_row("SELECT symbol FROM assets WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
    };
    Ok(match notice {
        // The quote asset the budget was stamped for, as Rails captures it when it enqueues the mail.
        Notice::EndOfFunds { quote_asset_id } => match (exchange_name, symbol(*quote_asset_id)?) {
            (Some(exchange_name), Some(quote)) => Some(end_of_funds(from, urls, &to, &exchange_name, &quote)),
            _ => None,
        },
        Notice::Error { error, .. } | Notice::StoppedByError { error } => exchange_name.map(|exchange_name| {
            let sentence = humanize_error(exchange_type.as_deref().unwrap_or_default(), &exchange_name, to.locale, &bounded(error, ERROR_LIMIT));
            failure(from, urls, &to, matches!(notice, Notice::StoppedByError { .. }), &label, &exchange_name, &sentence)
        }),
        Notice::StoppedByAmountLimit => symbol(settings.get("quote_asset_id").and_then(Value::as_i64))?
            .map(|quote| stopped_by_amount_limit(from, urls, &to, &label, &ruby_to_s(settings.get("quote_amount_limit")), &quote)),
    })
}
