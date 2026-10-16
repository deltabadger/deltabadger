//! Tracker::ImportsController: the dialog, and the answer to a submission without a file. Reading a
//! file is not served yet: a request that carries one, or a parked upload's token, is refused.
use super::{auth, layout, Ctx, WebError};
use crate::web::bots::{Segmented, SegmentedOption};
use askama::Template;
use axum::{extract::Extension, http::{header, StatusCode}, response::{IntoResponse, Response}};

/// Import::Run::FORMATS, in their order, and Import::Run::DEFAULT_FORMAT.
pub const FORMATS: [&str; 2] = ["deltabadger", "binance"];
pub const DEFAULT_FORMAT: &str = "deltabadger";

#[derive(Template)]
#[template(path = "tracker/import_new.html")]
struct New<'a> { ctx: &'a Ctx, csrf: String, formats: String, default_format: &'static str }

#[derive(Template)]
#[template(path = "tracker/import_create.html")]
struct Create<'a> { ctx: &'a Ctx, error: &'a str }

fn html(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

pub async fn new(Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)); }
    let options = FORMATS.iter().map(|&value| SegmentedOption {
        value, label: ctx.t(&format!("tracker.import.format_{value}")), active: value == DEFAULT_FORMAT, href: None,
    }).collect();
    let formats = Segmented { fluid: true, label: ctx.t("tracker.import.format"), key: None, options, links: false }.render()?;
    Ok(html(StatusCode::OK, New { ctx: &ctx, csrf: ctx.csrf_token(), formats, default_format: DEFAULT_FORMAT }.render()?))
}

/// Rails' `text.blank?` on the upload's binary string: empty, or ASCII whitespace only.
fn blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
}

/// `TOKEN = /\A[a-f0-9]{16}\z/`: the second request of the Binance/Binance.US question.
fn token(value: Option<&str>) -> bool {
    value.is_some_and(|t| t.len() == 16 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

pub async fn create(Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)); }
    let file = ctx.params.upload.as_ref().is_some_and(|(bytes, _)| !blank(bytes));
    if file || token(ctx.params.form("token")) { return Ok(layout::refused(&ctx, "tracker import")); }
    Ok(html(StatusCode::UNPROCESSABLE_ENTITY, Create { ctx: &ctx, error: "tracker.import.error_no_file" }.render()?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn blank_and_token_follow_rails() {
        assert!(super::blank(b"") && super::blank(b" \r\n\t\x0b\x0c"));
        assert!(!super::blank(b"\xef\xbb\xbf") && !super::blank(b"a"));
        assert!(super::token(Some("0123456789abcdef")));
        assert!(!super::token(Some("0123456789ABCDEF")) && !super::token(Some("0123456789abcde")) && !super::token(None));
    }
}
