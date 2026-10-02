//! Rails' flash as these pages use it. A message set during one request is shown by the next
//! response that renders the flash, and stays in the session across redirects until then.
//! `layouts/_flash.html.erb` is templates/layouts/_flash.html.
use super::session::Session;
use askama::Template;

pub const ALERT: &str = "alert";
pub const NOTICE: &str = "notice";

/// `flash[kind] = message`: replaces whatever was waiting, as Rails' sweep does once a request
/// touches the flash.
pub fn set(session: &Session, kind: &str, message: String) {
    session.lock().flash = vec![(kind.to_string(), message)];
}

/// The messages to render now: what the session carried, then `now` (`flash.now`), a later entry of
/// the same kind replacing an earlier one. Taking them empties the session's flash.
pub fn take(session: &Session, now: &[(&str, String)]) -> Vec<Message> {
    let mut entries: Vec<(String, String)> = std::mem::take(&mut session.lock().flash);
    for (kind, message) in now {
        entries.retain(|(k, _)| k != kind);
        entries.push((kind.to_string(), message.clone()));
    }
    entries.into_iter().map(|(kind, text)| Message { style: style(&kind), text }).collect()
}

fn style(kind: &str) -> &'static str {
    match kind {
        "alert" => "danger",
        "success" => "success",
        _ => "primary",
    }
}

pub struct Message {
    pub style: &'static str,
    pub text: String,
}

#[derive(Template)]
#[template(path = "layouts/_flash.html")]
struct Partial<'a> {
    messages: &'a [Message],
}

/// The markup of `render 'layouts/flash'`.
pub fn render(messages: &[Message]) -> Result<String, askama::Error> {
    Partial { messages }.render()
}
