//! The web UI (spec §4): server-rendered pages that match the Rails app's, for the compiled
//! JavaScript and CSS the Rails app ships. Rails is the oracle: tests/pages.rs renders every page in
//! both and compares.
pub mod assets;
pub mod i18n;
pub mod layout;
pub mod locale;
pub mod server;

use crate::crypto::{Cipher, EncryptionKeys};
use crate::engine::{Clock, EngineError};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, MethodRouter};
use axum::Router;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use sha2::Sha256;
use std::sync::{Arc, Mutex, PoisonError};
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
        Ok(Self(Arc::new(Inner { config, keys, cipher: Cipher::new(&encryption), clock, db: Mutex::new(primary) })))
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

/// Rails' `rails/health#show`.
async fn up() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
     "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>").into_response()
}

/// A route that exists for these methods only; any other method is a page this build does not serve.
fn only(route: MethodRouter<App>) -> MethodRouter<App> {
    route.fallback(layout::not_ported)
}

fn routes(app: App) -> Router {
    Router::new()
        .fallback(layout::not_ported)
        .route("/up", only(get(up)))
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
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn form(&self, name: &str) -> Option<&str> {
        self.form.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
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

#[derive(Clone)]
struct Entry {
    routes: Router,
}

/// Everything that has to happen before a route is chosen:
/// - a static file is answered at once, as Rails' static file server sits in front of the app;
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
    let form = Vec::new();
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
