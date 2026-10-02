//! What Turbo and the compiled JS expect from the server: `<turbo-stream>` elements exactly as
//! turbo-rails' tag builder writes them, the app's custom actions (`redirect`, `add_class`,
//! `remove_class`, app/javascript/controllers/application.js), and frame requests.
use super::i18n::escape;
use axum::http::HeaderMap;

/// `Mime[:turbo_stream]`, with the charset Rails adds to a response.
pub const CONTENT_TYPE: &str = "text/vnd.turbo-stream.html; charset=utf-8";

/// replace, update, append, prepend: `content` is trusted markup, `target` is escaped.
pub fn stream(action: &str, target: &str, content: &str) -> String {
    format!("<turbo-stream action=\"{}\" target=\"{}\"><template>{content}</template></turbo-stream>", escape(action), escape(target))
}

pub fn remove(target: &str) -> String {
    format!("<turbo-stream action=\"remove\" target=\"{}\"></turbo-stream>", escape(target))
}

/// SharedHelper#turbo_stream_page_refresh.
pub fn refresh() -> &'static str {
    "<turbo-stream action=\"refresh\"></turbo-stream>"
}

/// SharedHelper#turbo_stream_redirect: the target is the URL to visit.
pub fn redirect(url: &str) -> String {
    stream("redirect", url, "")
}

/// SharedHelper#turbo_stream_prepend_flash; `flash` is the markup of flash::render.
pub fn prepend_flash(flash: &str) -> String {
    stream("prepend", "flash", flash)
}

fn class_action(action: &str, target: &str, class_name: &str) -> String {
    format!("<turbo-stream class-name=\"{}\" action=\"{action}\" target=\"{}\"><template></template></turbo-stream>", escape(class_name), escape(target))
}

pub fn add_class(target: &str, class_name: &str) -> String {
    class_action("add_class", target, class_name)
}

pub fn remove_class(target: &str, class_name: &str) -> String {
    class_action("remove_class", target, class_name)
}

/// The id of the frame a request was made for; such a request gets turbo-rails' minimal frame layout.
pub fn frame(headers: &HeaderMap) -> Option<&str> {
    headers.get("turbo-frame").and_then(|v| v.to_str().ok())
}
