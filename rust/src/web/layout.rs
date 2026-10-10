//! Rendering: the per-request context handlers and templates read (`Ctx`), the devise layout and
//! turbo-rails' frame layout, redirects, and the answer for pages this build does not serve.
//! Templates live in rust/templates and were written from Rails' rendered output.
use super::auth::{Current, User};
use super::session::Session;
use super::{assets, csrf, flash, header_text, i18n, locale, normalize_path, turbo, App, Params, WebError};
use askama::Template;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// One request, as handlers and templates see it.
#[derive(Clone)]
pub struct Ctx {
    pub app: App,
    pub params: Arc<Params>,
    pub session: Session,
    pub current: Current,
    /// The request's method after `_method` was applied.
    pub method: Method,
    /// I18n.locale for this request (switch_locale).
    pub locale: &'static str,
    /// The value of `<meta name="csp-nonce">` and of the policy header.
    pub nonce: String,
    pub now: DateTime<Utc>,
    /// The id in a `Turbo-Frame` request header.
    pub turbo_frame: Option<String>,
}

pub struct Language {
    pub href: String,
    pub label: &'static str,
}

/// layouts/_language_dropdown.html.erb: the names are not translated, and English comes first.
const LANGUAGES: [(&str, &str); 15] = [
    ("en", "🇬🇧 English"), ("de", "🇩🇪 Deutsch"), ("nl", "🇳🇱 Nederlands"), ("fr", "🇫🇷 Français"), ("es", "🇪🇸 Español"),
    ("pt", "🇵🇹 Português"), ("it", "🇮🇹 Italiano"), ("pl", "🇵🇱 Polski"), ("ru", "🇷🇺 Русский"), ("cs", "🇨🇿 Čeština"),
    ("sk", "🇸🇰 Slovenčina"), ("da", "🇩🇰 Dansk"), ("sv", "🇸🇪 Svenska"), ("el", "🇬🇷 Ελληνικά"), ("bg", "🇧🇬 Български"),
];

impl Ctx {
    pub fn new(app: App, params: Arc<Params>, session: Session, current: Current, nonce: String, now: DateTime<Utc>, request: &Request) -> Self {
        let locale = locale::switch(params.locale(), current.user().and_then(|u| u.locale.as_deref()));
        let turbo_frame = turbo::frame(request.headers()).map(str::to_string);
        Self { app, params, session, current, method: request.method().clone(), locale, nonce, now, turbo_frame }
    }

    pub fn user(&self) -> Option<&User> {
        self.current.user()
    }

    /// The view helper `t` in this request's locale.
    pub fn t(&self, key: &str) -> String {
        i18n::t(self.locale, key, &[])
    }

    /// A route helper's path under default_url_options: prefixed unless the locale is the default.
    pub fn path(&self, path: &str) -> String {
        locale::path(self.locale, path)
    }

    pub fn asset(&self, logical: &str) -> &'static str {
        assets::path(logical)
    }

    /// `current_page?(some_path)`: a GET whose path is exactly what the route helper generates now.
    /// So `/en/bots`, and `/bots` for a user whose saved locale is not English, are not "current".
    pub fn current_page(&self, path: &str) -> bool {
        matches!(self.method, Method::GET | Method::HEAD) && self.path(path) == self.params.full_path
    }

    /// The masked CSRF token for this response; the session gets its token on first use, as in Rails.
    pub fn csrf_token(&self) -> String {
        let mut session = self.session.lock();
        csrf::masked(session.csrf.get_or_insert_with(csrf::new_token))
    }

    pub fn csrf_verified(&self, headers: &HeaderMap) -> bool {
        csrf::verified(self.session.lock().csrf.as_deref(), headers, self.params.authenticity_token(), self.app.config.origin(headers).as_deref())
    }

    /// The entries of the language dropdown. Which one is left out follows `params[:locale]`, not
    /// the locale in effect: without the parameter English is hidden, whatever the page is in.
    pub fn languages(&self) -> Vec<Language> {
        let param = self.params.locale();
        let query = locale::switch_query(&self.params.query.iter().filter(|(key,_)|!locale::sensitive_query(key)).cloned().collect::<Vec<_>>());
        LANGUAGES.iter()
            .filter(|(code, _)| if *code == "en" { param.is_some_and(|p| p != "en") } else { param != Some(code) })
            .map(|(code, label)| Language { href: locale::switch_path(code, &self.params.route_path, &query), label })
            .collect()
    }
}

/// A redirect with an empty body. Rails sends an absolute URL; a path is the same resource and does
/// not depend on knowing the public host.
pub fn redirect(status: StatusCode, location: &str) -> Response {
    let safe=locale::without_secret_query(location);
    let location=safe.as_ref();
    let mut response = (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")]).into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// A redirect issued by a filter that runs before ApplicationController#set_no_cache, so it carries
/// Rack's `no-cache` even for a signed-in user.
pub fn early_redirect(status: StatusCode, location: &str) -> Response {
    let mut response = redirect(status, location);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// ApplicationController#handle_unverified_request: say so and go back. Nobody is signed out. The
/// check runs before switch_locale, so the message is English and the fallback is the unprefixed root.
pub fn unverified_request(ctx: &Ctx, headers: &HeaderMap) -> Response {
    flash::set(&ctx.session, flash::ALERT, i18n::text(i18n::DEFAULT, "errors.unverified_request", &[]));
    early_redirect(StatusCode::FOUND, &back(&ctx.app.config, headers).unwrap_or_else(|| "/".to_string()))
}

/// `redirect_back`: the referer's path and query, when the referer is a page of this deployment's
/// own origin (scheme, host and port; Rails compares the host alone). The path is squeezed first, so
/// the result begins with exactly one slash: a `Location` of `//evil.test/path` would name another host.
pub fn back(config: &super::Config, headers: &HeaderMap) -> Option<String> {
    let referer = header_text(headers, "referer")?;
    let rest = referer.strip_prefix(config.origin(headers)?.as_str())?;
    // "http://bot.example" is also how "http://bot.example.evil.test/" and "http://bot.example@evil.test/" begin.
    // A browser reads a backslash in a Location as a slash, so `/\evil.test` would be `//evil.test`,
    // and it drops tabs and line breaks from a URL before reading it, so `/<tab>/evil.test` would be too.
    if !(rest.is_empty() || rest.starts_with(['/', '?'])) || rest.contains(|c: char| c == '\\' || c.is_ascii_control()) {
        return None;
    }
    let (path, query) = rest.split_once('?').map_or((rest, None), |(path, query)| (path, Some(query)));
    let path = normalize_path(&format!("/{path}"));
    Some(query.map_or(path.clone(), |query| format!("{path}?{query}")))
}

#[derive(Template)]
#[template(path = "layouts/devise.html")]
struct DeviseLayout<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    flash: &'a str,
    body: &'a str,
}

#[derive(Template)]
#[template(path = "layouts/frame.html")]
struct FrameLayout<'a> {
    csrf: &'a str,
    body: &'a str,
}

#[derive(Template)]
#[template(path = "not_ported.html")]
struct NotPorted<'a> {
    frame: &'a str,
    method: &'a str,
    path: &'a str,
}

/// A rendered view on its way into a layout.
pub struct Page<'a> {
    pub status: StatusCode,
    pub body: String,
    /// `flash.now`: shown in this response only.
    pub flash_now: Vec<(&'a str, String)>,
}

pub(super) fn html(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

/// A request made for a Turbo frame gets turbo-rails' frame layout: the CSRF meta tags and the view,
/// nothing else. The flash is not rendered, so it stays for the next full page.
pub(crate) fn frame(csrf: &str, page: &Page) -> Result<Response, WebError> {
    Ok(html(page.status, FrameLayout { csrf, body: &page.body }.render()?))
}

/// layouts/devise.html.erb.
pub fn devise(ctx: &Ctx, csrf: &str, page: Page) -> Result<Response, WebError> {
    if ctx.turbo_frame.is_some() {
        return frame(csrf, &page);
    }
    let flash = flash::render(&flash::take(&ctx.session, &page.flash_now))?;
    Ok(html(page.status, DeviseLayout { v: ctx, csrf, flash: &flash, body: &page.body }.render()?))
}

/// The answer for everything Rails serves and this build does not yet: 501, naming the request.
/// Never a redirect, so a missing page cannot pass for a working one. The message sits in a
/// `<turbo-frame>` with the id the request asked for, so Turbo shows it where the content would have gone.
pub fn not_ported_response(method: &Method, path: &str, frame: Option<&str>) -> Response {
    let safe=locale::without_secret_query(path);
    let page = NotPorted { frame: frame.unwrap_or("not-ported"), method: method.as_str(), path:&safe };
    match page.render() {
        Ok(body) => html(StatusCode::NOT_IMPLEMENTED, body),
        Err(error) => WebError::from(error).into_response(),
    }
}

/// A record that is not there for this user, where Rails lets ActiveRecord::RecordNotFound through:
/// 404 with public/404.html. (Rails in production then redirects to the root; a page that is not
/// there is not made to look like one that is.)
pub fn missing() -> Response {
    let body = assets::find("/404.html").map_or(&b"Not Found"[..], |file| file.body);
    let mut response = (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response();
    response.extensions_mut().insert(super::headers::BelowControllers);
    response
}

/// The same answer from a handler that knows why it does not serve a page Rails serves. The reason
/// goes to the log: the page names the request, which is what its reader can act on.
pub fn refused(ctx: &Ctx, reason: &str) -> Response {
    eprintln!("deltabadger: {} {} is not served: {reason}", ctx.method, locale::without_secret_query(&ctx.params.fullpath));
    not_ported_response(&ctx.method, &ctx.params.fullpath, ctx.turbo_frame.as_deref())
}

/// The answer to a failed read of a bot: the 501 page when it failed on a stored number this build
/// does not read (`bot::Unreadable`), and the failure itself otherwise.
pub fn or_refused(ctx: &Ctx, error: WebError) -> Result<Response, WebError> {
    if super::bot::unreadable(&error) { Ok(refused(ctx, super::bot::UNREADABLE)) } else { Err(error) }
}

pub async fn not_ported(request: Request) -> Response {
    // The path as it was requested: `entry` has taken the locale prefix off the URI by now.
    let path = request.extensions().get::<Arc<Params>>().map_or_else(|| request.uri().path().to_string(), |params| params.fullpath.clone());
    not_ported_response(request.method(), &path, header_text(request.headers(), "turbo-frame"))
}
