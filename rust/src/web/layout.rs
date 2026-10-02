//! The answer for pages this build does not serve.
use super::{header_text, Params, WebError};
use askama::Template;
use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

#[derive(Template)]
#[template(path = "not_ported.html")]
struct NotPorted<'a> {
    frame: &'a str,
    method: &'a str,
    path: &'a str,
}

fn html(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

/// The answer for everything Rails serves and this build does not yet: 501, naming the request.
/// Never a redirect, so a missing page cannot pass for a working one. The message sits in a
/// `<turbo-frame>` with the id the request asked for, so Turbo shows it where the content would have gone.
pub fn not_ported_response(method: &Method, path: &str, frame: Option<&str>) -> Response {
    let page = NotPorted { frame: frame.unwrap_or("not-ported"), method: method.as_str(), path };
    match page.render() {
        Ok(body) => html(StatusCode::NOT_IMPLEMENTED, body),
        Err(error) => WebError::from(error).into_response(),
    }
}

pub async fn not_ported(request: Request) -> Response {
    // The path as it was requested: `entry` has taken the locale prefix off the URI by now.
    let path = request.extensions().get::<Arc<Params>>().map_or_else(|| request.uri().path().to_string(), |params| params.fullpath.clone());
    not_ported_response(request.method(), &path, header_text(request.headers(), "turbo-frame"))
}
