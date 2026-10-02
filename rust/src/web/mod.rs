//! The web UI (spec §4): server-rendered pages that match the Rails app's, for the compiled
//! JavaScript and CSS the Rails app ships. Rails is the oracle: tests/pages.rs renders every page in
//! both and compares.
//!
//! A request passes through `entry` (static files, form parsing, method override, locale prefix),
//! then the routes, wrapped by `pipeline` (session, rate limits, who is signed in, CSRF, response headers).
pub mod assets;
pub mod auth;
pub mod bots;
pub mod cable;
pub mod csrf;
pub mod flash;
pub mod headers;
pub mod i18n;
pub mod layout;
pub mod locale;
pub mod rate_limit;
pub mod server;
pub mod session;
pub mod shell;
pub mod timezone;
pub mod turbo;

use crate::crypto::{Cipher, EncryptionKeys};
use crate::engine::{Clock, EngineError};
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, MethodRouter};
use axum::Router;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use sha2::Sha256;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tower::ServiceExt;

#[derive(Debug)]
pub enum WebError {
    Engine(EngineError),
    /// The environment this process was started with cannot serve this install.
    Config(String),
    Template(askama::Error),
    /// A blocking task (database, bcrypt) did not finish.
    Task(String),
}
impl From<EngineError> for WebError { fn from(e: EngineError) -> Self { Self::Engine(e) } }
impl From<rusqlite::Error> for WebError { fn from(e: rusqlite::Error) -> Self { Self::Engine(EngineError::Sqlite(e)) } }
impl From<askama::Error> for WebError { fn from(e: askama::Error) -> Self { Self::Template(e) } }

/// The body of a 500 response: public/500.html, as Rails answers an exception in production.
fn error_page() -> &'static [u8] {
    assets::find("/500.html").map_or(&b"Internal Server Error"[..], |file| file.body)
}

impl IntoResponse for WebError {
    /// A request that fails is answered and logged; it never takes the process down.
    fn into_response(self) -> Response {
        eprintln!("deltabadger: request failed: {self:?}");
        (StatusCode::INTERNAL_SERVER_ERROR, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], error_page()).into_response()
    }
}

/// config/application.rb `env_boolean`: the spellings an operator may have written; anything else is no answer.
fn env_boolean(value: Option<String>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "t" | "true" | "y" | "yes" | "on" => Some(true),
        "0" | "f" | "false" | "n" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// An origin as a browser writes one in an `Origin` header: scheme and host in lower case, and no
/// port when it is the scheme's default. `https://Bot.Example:443` is `https://bot.example`.
fn canonical_origin(scheme: &str, authority: &str) -> String {
    let (scheme, authority) = (scheme.to_ascii_lowercase(), authority.to_ascii_lowercase());
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    format!("{scheme}://{}", authority.strip_suffix(default_port).unwrap_or(&authority))
}

pub struct Config {
    pub secret_key_base: String,
    /// `force_ssl_from_env`: FORCE_SSL, else whether APP_ROOT_URL is https. Decides `Secure` and HSTS.
    pub force_ssl: bool,
    /// `behind_proxy_from_env`: BEHIND_PROXY, else the same signal as `force_ssl`.
    pub behind_proxy: bool,
    /// The origin (`scheme://host[:port]`) of APP_ROOT_URL, when that is set.
    pub own_origin: Option<String>,
    /// MarketDataSettings.deltabadger_available?: a hosted install, where stock trading is always on.
    pub market_data_url: bool,
}

impl Config {
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, WebError> {
        let present = |name: &str| env(name).filter(|v| !v.trim().is_empty());
        let secret_key_base = present("SECRET_KEY_BASE").ok_or_else(|| WebError::Config("SECRET_KEY_BASE is not set".into()))?;
        let app_root_url = present("APP_ROOT_URL");
        let force_ssl = env_boolean(env("FORCE_SSL")).unwrap_or_else(|| app_root_url.as_deref().is_some_and(|url| url.trim().to_ascii_lowercase().starts_with("https://")));
        // "https://bot.example.com/", "https://bot.example.com/path" and "https://Bot.Example.com:443" all name the origin "https://bot.example.com".
        let own_origin = app_root_url.as_deref().and_then(|url| {
            let (scheme, rest) = url.trim().split_once("://")?;
            Some(canonical_origin(scheme, rest.split(['/', '?', '#']).next().unwrap_or(rest)))
        });
        Ok(Self {
            secret_key_base,
            force_ssl,
            behind_proxy: env_boolean(env("BEHIND_PROXY")).unwrap_or(force_ssl),
            own_origin,
            market_data_url: present("MARKET_DATA_URL").is_some(),
        })
    }

    /// The origin a request was made to, as Rails' `request.base_url` derives it: the `Host` header,
    /// under https when SSL is forced. A scheme a proxy forwarded is not consulted.
    pub fn request_origin(&self, headers: &HeaderMap) -> Option<String> {
        header_text(headers, "host").map(|host| canonical_origin(if self.force_ssl { "https" } else { "http" }, host))
    }

    /// The origin this deployment's pages have: APP_ROOT_URL's when it is set, else the request's.
    pub fn origin(&self, headers: &HeaderMap) -> Option<String> {
        self.own_origin.clone().or_else(|| self.request_origin(headers))
    }
}

/// Keys for this crate's own cookie and signatures: HMAC-SHA256(secret_key_base, label). The labels
/// are Rust-specific, so nothing Rails signed or encrypted is accepted here, and the reverse.
pub struct Keys {
    pub session: [u8; 32],
    pub streams: [u8; 32],
}

fn derive(secret: &str, label: &str) -> Result<[u8; 32], WebError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).map_err(|e| WebError::Config(e.to_string()))?;
    mac.update(label.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

pub struct Inner {
    pub config: Config,
    pub keys: Keys,
    pub cipher: Cipher,
    pub clock: Arc<dyn Clock + Send + Sync>,
    pub limiter: rate_limit::Limiter,
    pub hub: cable::Hub,
    /// Action Cable's BEAT_INTERVAL: 3 seconds. Shorter in tests.
    pub cable_ping: Duration,
    /// How often an open /cable connection is asked whether its session would still open one: 60
    /// seconds. Shorter in tests.
    pub cable_recheck: Duration,
    /// The web side's own connection (the engine owns another).
    /// ponytail: one connection behind a mutex; a small pool if one user's requests ever queue.
    db: Mutex<Connection>,
}

#[derive(Clone)]
pub struct App(Arc<Inner>);

impl std::ops::Deref for App {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.0 }
}

impl App {
    /// `primary` is a connection `store::open` returned, so the install has passed `store::check`.
    pub fn new(config: Config, env: &dyn Fn(&str) -> Option<String>, primary: Connection, clock: Arc<dyn Clock + Send + Sync>) -> Result<Self, WebError> {
        let encryption = EncryptionKeys::resolve(env, &config.secret_key_base).map_err(|e| WebError::Config(format!("{e:?}")))?;
        let keys = Keys { session: derive(&config.secret_key_base, "deltabadger rust session v1")?, streams: derive(&config.secret_key_base, "deltabadger rust turbo streams v1")? };
        Ok(Self(Arc::new(Inner {
            config, keys, cipher: Cipher::new(&encryption), clock, limiter: rate_limit::Limiter::default(), hub: cable::Hub::default(),
            cable_ping: Duration::from_secs(3), cable_recheck: Duration::from_secs(60), db: Mutex::new(primary),
        })))
    }

    /// For tests: the same app with other /cable intervals. Only before the app is shared.
    pub fn with_cable_timing(self, ping: Duration, recheck: Duration) -> Result<Self, WebError> {
        let mut inner = Arc::try_unwrap(self.0).map_err(|_| WebError::Config("the app is already shared".into()))?;
        (inner.cable_ping, inner.cable_recheck) = (ping, recheck);
        Ok(Self(Arc::new(inner)))
    }

    pub fn now(&self) -> DateTime<Utc> { self.clock.now() }

    /// Runs database work (and bcrypt) off the runtime's thread: rusqlite is synchronous, and a
    /// handler must not hold up the other tasks of this process, the engine loop among them.
    pub async fn db<T: Send + 'static>(&self, work: impl FnOnce(&Connection) -> Result<T, WebError> + Send + 'static) -> Result<T, WebError> {
        let app = self.clone();
        tokio::task::spawn_blocking(move || work(&app.0.db.lock().unwrap_or_else(PoisonError::into_inner)))
            .await
            .map_err(|e| WebError::Task(e.to_string()))?
    }
}

/// A request header as text.
pub fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Rails' `rails/health#show`, with the headers the middleware around it adds.
async fn up(State(app): State<App>) -> Response {
    let mut response = (StatusCode::OK, [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>").into_response();
    headers::policy(response.headers_mut(), &headers::new_nonce(), app.config.force_ssl);
    headers::controller_defaults(response.headers_mut(), StatusCode::OK, false);
    response
}

/// CspReportsController: a browser posts policy violations here on its own, with no CSRF token.
/// Accepted and dropped.
/// ponytail: Rails logs nine sanitised fields of each report; port that when the policy is enforced.
async fn csp_report() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// A route that exists for these methods only; any other method is a page this build does not serve.
fn only(route: MethodRouter<App>) -> MethodRouter<App> {
    route.fallback(layout::not_ported)
}

fn routes(app: App) -> Router {
    Router::new()
        .route("/", only(get(bots::home)))
        .route("/login", only(get(auth::new).post(auth::create)))
        .route("/logout", only(delete(auth::destroy)))
        .route("/verify_two_factor", only(get(auth::two_factor).post(auth::two_factor)))
        .route("/bots", only(get(bots::index)))
        .fallback(layout::not_ported)
        .layer(middleware::from_fn_with_state(app.clone(), pipeline))
        // Outside the pipeline, as in Rails: no session, no CSRF check, no rate limit.
        .route("/up", only(get(up)))
        .route("/csp-report", only(post(csp_report)))
        .route("/cable", get(cable::connect))
        .with_state(app)
}

/// What `entry` learned about the request before routing.
pub struct Params {
    /// The path as requested, slashes squeezed, locale prefix included: Rails' `request.path`.
    pub full_path: String,
    /// `full_path` with the query string: Rails' `request.fullpath`.
    pub fullpath: String,
    /// The path the routes match: `full_path` without its locale prefix.
    pub route_path: String,
    pub path_locale: Option<&'static str>,
    pub query: Vec<(String, String)>,
    /// The fields of an `application/x-www-form-urlencoded` POST body, by their literal names (`user[email]`).
    pub form: Vec<(String, String)>,
}

impl Params {
    /// The last value of a repeated key, as Rack reads a query string.
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().rfind(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn form(&self, name: &str) -> Option<&str> {
        // The last value of a repeated name, as Rack: `check_box` sends a hidden "0" and then the box's "1".
        self.form.iter().rfind(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// `params[:locale].presence`: the path prefix, else the form field, else the query parameter.
    pub fn locale(&self) -> Option<&str> {
        self.path_locale.or_else(|| self.form("locale")).or_else(|| self.query("locale")).filter(|v| !v.trim().is_empty())
    }
}

/// RackAttackPaths.normalize, which is also how Rails' router reads a path: repeated slashes are one,
/// and a trailing slash is dropped.
pub fn normalize_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

fn pairs(encoded: &[u8]) -> Vec<(String, String)> {
    form_urlencoded::parse(encoded).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

/// Rack::MethodOverride: the method a form's `_method` field asks for, when it is one a form may ask
/// for. A repeated field is read as Rack reads it: the last value.
pub fn method_override(form: &[(String, String)]) -> Option<Method> {
    let (_, method) = form.iter().rfind(|(name, _)| name == "_method")?;
    match method.to_ascii_uppercase().as_str() {
        "PATCH" => Some(Method::PATCH),
        "PUT" => Some(Method::PUT),
        "DELETE" => Some(Method::DELETE),
        _ => None,
    }
}

#[derive(Clone)]
struct Entry {
    routes: Router,
}

/// The largest form body `entry` reads.
const FORM_LIMIT: usize = 1024 * 1024;

/// Everything that has to happen before a route is chosen:
/// - a static file is answered at once, as Rails' static file server sits in front of the app;
/// - a form POST is parsed, and its `_method` field (how Turbo and `button_to` send PATCH, PUT and
///   DELETE) replaces the method, as Rack::MethodOverride does;
/// - the path is normalised and its locale prefix taken off, so one set of routes serves `/login`
///   and `/de/login`.
async fn entry(State(entry): State<Entry>, request: Request) -> Response {
    let (mut parts, body) = request.into_parts();
    let full_path = normalize_path(parts.uri.path());
    if matches!(parts.method, Method::GET | Method::HEAD) {
        if let Some(file) = assets::find(&full_path) {
            return assets::respond(file, parts.method == Method::HEAD);
        }
    }
    let query = parts.uri.query().map(|q| pairs(q.as_bytes())).unwrap_or_default();
    let form_post = parts.method == Method::POST && header_text(&parts.headers, "content-type").is_some_and(|t| t.starts_with("application/x-www-form-urlencoded"));
    let (form, body) = if form_post {
        match axum::body::to_bytes(body, FORM_LIMIT).await {
            Ok(bytes) => (pairs(&bytes), Body::empty()),
            Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Form too large\n").into_response(),
        }
    } else {
        (Vec::new(), body)
    };
    if let Some(method) = method_override(&form) {
        parts.method = method;
    }
    let with_query = |path: &str| parts.uri.query().map_or_else(|| path.to_string(), |q| format!("{path}?{q}"));
    let fullpath = with_query(&full_path);
    let (path_locale, route_path) = locale::split(&full_path);
    let route_path = route_path.to_string();
    let Ok(uri) = with_query(&route_path).parse::<Uri>() else { return (StatusCode::BAD_REQUEST, "Bad Request\n").into_response() };
    parts.uri = uri;
    parts.extensions.insert(Arc::new(Params { full_path, fullpath, route_path, path_locale, query, form }));
    match entry.routes.oneshot(Request::from_parts(parts, body)).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

/// The whole web application as one service. `serve` binds it; tests call it with `oneshot`.
pub fn router(app: App) -> Router {
    Router::new().fallback(entry).with_state(Entry { routes: routes(app) })
}

/// What goes onto every response of the app: the session cookie when it has to be written, the
/// policy headers, and the controller's default headers unless the response was produced below the
/// controllers. The cookie is written when the request changed the session (`before` is what the
/// request arrived with), and at no other time. Rails writes it on every response; see web::session
/// for why this crate does not.
fn finish(app: &App, session: &session::Session, before: &session::SessionData, nonce: &str, now: DateTime<Utc>, signed_in: bool, mut response: Response) -> Response {
    let data = session.snapshot();
    if data != *before {
        response.headers_mut().append(header::SET_COOKIE, session::set_cookie(&session::seal(&app.keys.session, &data, now), now, app.config.force_ssl));
    }
    headers::policy(response.headers_mut(), nonce, app.config.force_ssl);
    let status = response.status();
    if response.extensions().get::<headers::BelowControllers>().is_some() {
        headers::cache_control(response.headers_mut(), status, false);
    } else {
        headers::controller_defaults(response.headers_mut(), status, signed_in);
    }
    response
}

/// What ApplicationController and the middleware below it do around every action, in Rails' order.
async fn pipeline(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let now = app.now();
    let nonce = headers::new_nonce();
    let Some(params) = request.extensions().get::<Arc<Params>>().cloned() else {
        return WebError::Config("a request reached the routes without passing web::router's entry".into()).into_response();
    };
    let before = session::from_request(&app.keys.session, request.headers(), now).map(|opened| opened.data).unwrap_or_default();
    let session = session::Session::new(before.clone());

    // rack-attack: after the session middleware, before everything else.
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip());
    let address = rate_limit::client_key(&app.config, request.headers(), peer);
    if let Some(retry_after) = app.limiter.hit(request.method(), &params.route_path, &address, now) {
        return finish(&app, &session, &before, &nonce, now, false, rate_limit::throttled(retry_after));
    }

    let current = match auth::current_user(&app, &session, now).await {
        Ok(current) => current,
        Err(error) => return error.into_response(),
    };
    let signed_in = matches!(current, auth::Current::SignedIn(_));
    let context = layout::Ctx::new(app.clone(), params, session.clone(), current, nonce.clone(), now, &request);

    let response = if let Some(early) = auth::prepended_filters(&context) {
        early
    } else if !matches!(*request.method(), Method::GET | Method::HEAD) && !context.csrf_verified(request.headers()) {
        layout::unverified_request(&context, request.headers())
    } else if let Some(inactive) = auth::inactive(&context) {
        inactive
    } else {
        request.extensions_mut().insert(context);
        next.run(request).await
    };
    finish(&app, &session, &before, &nonce, now, signed_in, response)
}
