//! The web UI: server-rendered pages that match the Rails app's, for the compiled
//! JavaScript and CSS the Rails app ships. Rails is the oracle: tests/pages.rs renders every page in
//! both and compares.
//!
//! A request passes through `entry` (static files, form parsing, method override, locale prefix),
//! then the routes, wrapped by `pipeline` (session, rate limits, who is signed in, CSRF, response headers).
pub mod assets;
pub mod auth;
pub mod bearer;
pub mod bot;
pub mod bots;
pub mod cable;
pub mod consent;
pub mod colors;
pub mod csrf;
pub mod flash;
pub mod format;
pub mod headers;
pub mod i18n;
pub mod layout;
pub mod locale;
pub mod oauth;
pub mod rate_limit;
pub mod ring;
pub mod server;
pub mod session;
pub mod shell;
pub mod timezone;
pub mod turbo;

use crate::crypto::{hash_password, verify_password, Cipher, EncryptionKeys};
use crate::engine::{Clock, EngineError};
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, MethodRouter};
use axum::Router;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use sha2::Sha256;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Semaphore;
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
pub(crate) fn env_boolean(value: Option<String>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "t" | "true" | "y" | "yes" | "on" => Some(true),
        "0" | "f" | "false" | "n" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// An origin as a browser writes one in an `Origin` header: scheme and host in lower case, and no
/// port when it is the scheme's default. `https://Bot.Example:443` is `https://bot.example`.
pub(crate) fn canonical_origin(scheme: &str, authority: &str) -> String {
    let (scheme, authority) = (scheme.to_ascii_lowercase(), authority.to_ascii_lowercase());
    let default_port = if matches!(scheme.as_str(), "https" | "wss") { ":443" } else { ":80" };
    format!("{scheme}://{}", authority.strip_suffix(default_port).unwrap_or(&authority))
}

/// A request header as Rack's env holds it: repeated lines are one value, joined by Puma.
pub(crate) fn joined(headers: &HeaderMap, name: &str) -> Option<String> {
    let lines: Vec<&str> = headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
    (!lines.is_empty()).then(|| lines.join(", "))
}

/// Rack::Request::ALLOWED_SCHEMES: what a forwarded header may name. Anything else is not read.
fn allowed_scheme(scheme: &str) -> Option<&'static str> {
    ["https", "http", "wss", "ws"].into_iter().find(|allowed| *allowed == scheme)
}

/// Rack::Utils.forwarded_values for `proto`: every `proto` of a `Forwarded` header, in order. `None`
/// when Rack reads nothing from the header: a parameter that is not by, for, host or proto.
fn forwarded_protos(header: &str) -> Option<Vec<String>> {
    let skip = |text: &str| text.trim_start_matches([' ', '\t', ';', ',']).to_string();
    let (mut rest, mut protos, mut parameters, mut escapes) = (skip(header), Vec::new(), 0, 0);
    while let Some((name, after)) = rest.split_once('=') {
        let name = name.trim().to_ascii_lowercase();
        parameters += 1;
        if parameters > 1024 || !["by", "for", "host", "proto"].contains(&name.as_str()) {
            return None;
        }
        let (value, after) = match after.strip_prefix('"') {
            // Quoted: up to the closing quote, a backslash taking the character after it as it is.
            Some(mut quoted) => {
                let mut value = String::new();
                while let Some((before, tail)) = quoted.split_once(['"', '\\']) {
                    value.push_str(before);
                    let closing = quoted.as_bytes().get(before.len()) == Some(&b'"');
                    quoted = tail;
                    if closing {
                        break;
                    }
                    escapes += 1;
                    if escapes > 1024 {
                        return None;
                    }
                    let mut characters = quoted.chars();
                    value.extend(characters.next());
                    quoted = characters.as_str();
                }
                (value, quoted)
            }
            // Unquoted: up to the next `;` or `,`, the separator left for `skip`.
            None => match after.find([';', ',']).and_then(|at| after.split_at_checked(at)) {
                Some((value, after)) => (value.trim().to_string(), after),
                None => (after.trim().to_string(), ""),
            },
        };
        if name == "proto" {
            protos.push(value);
        }
        rest = skip(after);
    }
    Some(protos)
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
    /// production.rb's `config.hosts`: the hosts a request may name, from ALLOWED_HOSTS. Empty when
    /// that is not set, and then every host is served, as in Rails (`config.hosts.clear`).
    pub allowed_hosts: Vec<String>,
}

/// `config.hosts` as config/environments/production.rb builds it: nothing when ALLOWED_HOSTS is
/// blank; else its comma-separated entries, each stripped (String#split drops empty entries at the
/// end, and keeps one in the middle), and then `localhost` and `127.0.0.1`.
pub fn allowed_hosts(value: Option<&str>) -> Vec<String> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else { return Vec::new() };
    let mut entries: Vec<&str> = value.split(',').collect();
    while entries.last().is_some_and(|entry| entry.is_empty()) {
        entries.pop();
    }
    entries.into_iter().map(|entry| entry.trim_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r', '\0']).to_string())
        .chain(["localhost".to_string(), "127.0.0.1".to_string()]).collect()
}

/// ActionDispatch::HostAuthorization::Permissions#allows? for one entry that is a string, which is
/// all ALLOWED_HOSTS can give: the host is the entry, in any case, and may add a port unless the
/// entry names its own; an entry that starts with a dot also takes one label in front of it.
/// (Rails builds `/\A<entry>(?::\d+)?\z/i`. The same is decided here without a pattern.)
pub fn host_allowed(entry: &str, host: &str) -> bool {
    let port = |text: &str| text.rsplit_once(':').filter(|(_, digits)| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).map(|(before, _)| before.len());
    let host = if port(entry).is_some() { host } else { port(host).map_or(host, |length| &host[..length]) };
    let Some(domain) = entry.strip_prefix('.') else { return host.eq_ignore_ascii_case(entry) };
    if host.eq_ignore_ascii_case(domain) {
        return true;
    }
    // One label of letters, digits and hyphens, and its dot.
    let label = host.len().checked_sub(domain.len() + 1).filter(|&length| length > 0);
    // The dot is looked at first: a byte that is a dot has a character boundary on either side.
    label.is_some_and(|length| host.as_bytes()[length] == b'.' && host[length + 1..].eq_ignore_ascii_case(domain)
        && host[..length].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// The last entry of `X-Forwarded-Host`, as Rails takes it (`split(/,\s?/).last`: one space after a
/// comma belongs to the comma, and empty entries at the end are dropped). `None` without the header
/// or with a blank one; `Some(None)` when it names nothing.
fn last_forwarded_host(headers: &HeaderMap) -> Option<Option<String>> {
    let hosts = joined(headers, "x-forwarded-host").filter(|hosts| !hosts.trim().is_empty())?;
    Some(hosts.split(',').map(|host| host.strip_prefix([' ', '\t']).unwrap_or(host)).rfind(|host| !host.is_empty()).map(str::to_string))
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
            allowed_hosts: allowed_hosts(env("ALLOWED_HOSTS").as_deref()),
        })
    }

    /// The scheme of a request as the Rails app sees it in production (pinned by the `base_url`
    /// vectors of tests/web.rs), from first to last:
    /// - https when SSL is forced: ActionDispatch::AssumeSSL, which production.rb turns on with force_ssl;
    /// - Rack::Request#scheme: `X-Forwarded-Ssl: on`, then the last `proto` of `Forwarded`, then the
    ///   last entry of `X-Forwarded-Proto`, then of `X-Forwarded-Scheme`, that names an allowed scheme;
    /// - Puma's `rack.url_scheme`: https when `X-Forwarded-Proto` begins with it or
    ///   `X-Forwarded-Scheme` is it, else http.
    ///
    /// Rails believes these headers from any peer, and so does this: they decide which `Origin` a
    /// form may come from, and a page on another site can set neither them nor its own `Origin`.
    pub fn request_scheme(&self, headers: &HeaderMap) -> &'static str {
        if self.force_ssl || joined(headers, "x-forwarded-ssl").as_deref() == Some("on") {
            return "https";
        }
        let last_allowed = |name: &str| joined(headers, name).and_then(|value| value.split([',', ' ', '\t']).rev().find_map(allowed_scheme));
        let (proto, scheme) = (joined(headers, "x-forwarded-proto"), joined(headers, "x-forwarded-scheme"));
        joined(headers, "forwarded").and_then(|header| forwarded_protos(&header)).and_then(|protos| allowed_scheme(protos.last()?))
            .or_else(|| last_allowed("x-forwarded-proto"))
            .or_else(|| last_allowed("x-forwarded-scheme"))
            .unwrap_or(if proto.is_some_and(|value| value.starts_with("https")) || scheme.as_deref() == Some("https") { "https" } else { "http" })
    }

    /// The origin a request was made to: Rails' `request.base_url`. The scheme is `request_scheme`;
    /// the host is the last entry of `X-Forwarded-Host` when a proxy sent one, else the `Host` header
    /// (ActionDispatch::Http::URL#raw_host_with_port).
    pub fn request_origin(&self, headers: &HeaderMap) -> Option<String> {
        let forwarded = last_forwarded_host(headers);
        let host = match &forwarded {
            Some(host) => host.as_deref()?,
            None => header_text(headers, "host")?,
        };
        Some(canonical_origin(self.request_scheme(headers), host))
    }

    /// ActionDispatch::HostAuthorization, the first middleware of the Rails app when ALLOWED_HOSTS is
    /// set: the hosts of this request that are not allowed, which are its `Host` and the last entry
    /// of its `X-Forwarded-Host`. Both are checked because Rails builds URLs from the second when it
    /// is there. A request with any is answered by `blocked_host` and reaches nothing else.
    pub fn blocked_hosts(&self, headers: &HeaderMap) -> Vec<String> {
        if self.allowed_hosts.is_empty() {
            return Vec::new();
        }
        let allowed = |host: &str| self.allowed_hosts.iter().any(|entry| host_allowed(entry, host));
        let mut blocked = Vec::new();
        // Rack's HTTP_HOST: no header is no host, and two lines are one value that matches nothing.
        let host = joined(headers, "host");
        if !host.as_deref().is_some_and(allowed) {
            blocked.push(host.unwrap_or_default());
        }
        match last_forwarded_host(headers) {
            Some(Some(forwarded)) if !forwarded.trim().is_empty() && !allowed(&forwarded) => blocked.push(forwarded),
            // A header that names no host at all (`,`): Rails fails on it with a 500. Refused here.
            Some(None) => blocked.push(String::new()),
            _ => {}
        }
        blocked
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
    /// The bcrypt computations of sign-in attempts in progress: `PASSWORD_CHECKS_AT_ONCE` at a time.
    password_slots: Arc<Semaphore>,
    /// Attempts waiting for one of those slots: at most `PASSWORD_CHECKS_WAITING`.
    password_waiting: AtomicUsize,
    /// For tests: called on the blocking thread before each bcrypt computation, so a test can count
    /// them and hold one.
    password_hook: Option<PasswordHook>,
    /// The web side's own connection (the engine owns another).
    /// ponytail: one connection behind a mutex; a small pool if one user's requests ever queue.
    db: Mutex<Connection>,
    /// The wake handle of the engine in this process, once `supervisor::serve` attaches it. Empty when the app runs
    /// alone (every router test).
    engine: std::sync::OnceLock<Arc<tokio::sync::Notify>>,
}

pub type PasswordHook = Arc<dyn Fn() + Send + Sync>;

/// bcrypt at cost 11 keeps a core busy for a good part of a second. Two at a time leaves the rest of
/// the blocking pool to the database work of every other request; eight more may wait (a few
/// seconds at worst), and an attempt beyond that is refused at once instead of queueing without bound.
pub const PASSWORD_CHECKS_AT_ONCE: usize = 2;
pub const PASSWORD_CHECKS_WAITING: usize = 8;

#[derive(Clone)]
pub struct App(Arc<Inner>);

impl std::ops::Deref for App {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.0 }
}

impl App {
    /// `primary` is a connection `store::open` returned, so the install has passed `store::check`.
    pub fn new(config: Config, env: &dyn Fn(&str) -> Option<String>, primary: Connection, clock: Arc<dyn Clock + Send + Sync>) -> Result<Self, WebError> {
        assets::require()?;
        let encryption = EncryptionKeys::resolve(env, &config.secret_key_base).map_err(|e| WebError::Config(format!("{e:?}")))?;
        let keys = Keys { session: derive(&config.secret_key_base, "deltabadger rust session v1")?, streams: derive(&config.secret_key_base, "deltabadger rust turbo streams v1")? };
        Ok(Self(Arc::new(Inner {
            config, keys, cipher: Cipher::new(&encryption), clock, limiter: rate_limit::Limiter::default(), hub: cable::Hub::default(),
            cable_ping: Duration::from_secs(3), cable_recheck: Duration::from_secs(60),
            password_slots: Arc::new(Semaphore::new(PASSWORD_CHECKS_AT_ONCE)), password_waiting: AtomicUsize::new(0), password_hook: None,
            db: Mutex::new(primary),
            engine: std::sync::OnceLock::new(),
        })))
    }

    /// For tests: the same app with other /cable intervals. Only before the app is shared.
    pub fn with_cable_timing(self, ping: Duration, recheck: Duration) -> Result<Self, WebError> {
        let mut inner = Arc::try_unwrap(self.0).map_err(|_| WebError::Config("the app is already shared".into()))?;
        (inner.cable_ping, inner.cable_recheck) = (ping, recheck);
        Ok(Self(Arc::new(inner)))
    }

    pub fn now(&self) -> DateTime<Utc> { self.clock.now() }

    /// Called once, by `supervisor::serve`, before the first request is served. A second call is ignored.
    pub fn attach_engine(&self, wake: Arc<tokio::sync::Notify>) {
        let _ = self.engine.set(wake);
    }

    /// After a committed write the engine must act on (a bot started, stopped, deleted or archived, or its settings
    /// saved): the engine re-reads its bots now instead of within a minute. Call immediately after a successful
    /// commit inside the `db` job, so dropping the HTTP future cannot lose the wake. One permit is stored,
    /// so a call while the engine is mid-pass makes it pass again right after;
    /// wakes coalesce. With no engine attached it does nothing.
    pub fn wake_engine(&self) {
        if let Some(wake) = self.engine.get() {
            wake.notify_one();
        }
    }

    /// For tests: the same app with a hook before each bcrypt computation. Only before the app is shared.
    pub fn with_password_hook(self, hook: PasswordHook) -> Result<Self, WebError> {
        let mut inner = Arc::try_unwrap(self.0).map_err(|_| WebError::Config("the app is already shared".into()))?;
        inner.password_hook = Some(hook);
        Ok(Self(Arc::new(inner)))
    }

    /// How many sign-in attempts are waiting for a bcrypt slot.
    pub fn password_checks_waiting(&self) -> usize {
        self.password_waiting.load(Ordering::SeqCst)
    }

    /// The one bcrypt computation of a sign-in attempt, off the runtime's thread and outside the
    /// database lock: `password` against `stored_hash`, or, with no hash (an unknown email), a hash
    /// computed and thrown away, so that both cost the same. `None` when `PASSWORD_CHECKS_WAITING`
    /// attempts are already waiting: the caller refuses the request.
    pub(crate) async fn password_check(&self, password: String, stored_hash: Option<String>) -> Result<Option<bool>, WebError> {
        struct Waiting<'a>(&'a AtomicUsize);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); }
        }
        let permit = match self.password_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let ahead = self.password_waiting.fetch_add(1, Ordering::SeqCst);
                let _waiting = Waiting(&self.password_waiting); // also when this request is dropped while it waits
                if ahead >= PASSWORD_CHECKS_WAITING {
                    return Ok(None);
                }
                self.password_slots.clone().acquire_owned().await.map_err(|e| WebError::Task(e.to_string()))?
            }
        };
        let hook = self.password_hook.clone();
        // The slot goes with the computation, not with this request: a client that hangs up does not free it early.
        let work = move || {
            let _permit = permit;
            if let Some(hook) = hook { hook() }
            match stored_hash {
                Some(hash) => verify_password(&password, &hash),
                // Only the work counts. Without a salt from the system there is no hash: the same failed sign-in.
                None => { let _ = hash_password(&password); false }
            }
        };
        tokio::task::spawn_blocking(work).await.map(Some).map_err(|e| WebError::Task(e.to_string()))
    }

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

/// What an OAuth client calls on its own (web::oauth). These routes are outside the pipeline: they
/// never read the session and take no CSRF token, because nothing in them acts for a signed-in
/// browser. That holds for exactly these paths and methods; any other method on them is the 501.
fn oauth_api(app: App) -> Router<App> {
    Router::new()
        .route("/.well-known/oauth-authorization-server", only(get(oauth::authorization_server)))
        .route("/.well-known/oauth-protected-resource", only(get(oauth::protected_resource)))
        .route("/.well-known/{*document}", any(oauth::absent))
        .route("/oauth/register", only(post(oauth::register)))
        .route("/oauth/token", only(post(oauth::token)))
        .route("/oauth/revoke", only(post(oauth::revoke)))
        // Doorkeeper also routes these two. The metadata does not advertise them and no client is told of them: not served.
        .route("/oauth/introspect", any(layout::not_ported))
        .route("/oauth/token/info", any(layout::not_ported))
        .layer(middleware::from_fn_with_state(app, oauth::api))
}

fn routes(app: App) -> Router {
    Router::new()
        .route("/", only(get(bots::home)))
        .route("/login", only(get(auth::new).post(auth::create)))
        .route("/logout", only(delete(auth::destroy)))
        .route("/verify_two_factor", only(get(auth::two_factor).post(auth::two_factor)))
        .route("/bots", only(get(bots::index)))
        .route("/oauth/authorize", only(get(consent::new).post(consent::create).delete(consent::destroy)))
        // The new-bot wizard, not served yet; named so that it is not read as a bot's id.
        .route("/bots/new", axum::routing::any(layout::not_ported))
        .route("/bots/{id}", only(get(bot::page::show)))
        .route("/bots/{id}/chart", only(get(bot::page::chart_frame)))
        .fallback(layout::not_ported)
        .layer(middleware::from_fn_with_state(app.clone(), pipeline))
        .merge(oauth_api(app.clone()))
        // Outside the pipeline, as in Rails: no session, no CSRF check, no rate limit.
        .route("/up", only(get(up)))
        .route("/csp-report", only(post(csp_report)))
        .route("/cable", only(get(cable::connect)))
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
    /// The fields of an `application/x-www-form-urlencoded` write body, by their literal names (`user[email]`).
    pub form: Vec<(String, String)>,
    /// A JSON object on an OAuth POST or a supported existing-bot action.
    pub json: Option<serde_json::Value>,
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

    /// Only a scalar token is considered. Structured JSON never becomes a string token.
    pub fn authenticity_token(&self) -> Option<&str> {
        match self.json.as_ref().and_then(|v| v.get("authenticity_token")) {
            Some(value) => value.as_str(),
            None => self.form("authenticity_token"),
        }
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
    app: App,
    routes: Router,
    body_read_timeout: Duration,
}

/// HostAuthorization's DefaultResponseApp as production runs it: 403 with no body, `text/plain` to
/// an XMLHttpRequest and `text/html` to anything else, and one line in the log. Nothing else of the
/// app has seen the request: there are no other headers and no cookie.
pub fn blocked_host(headers: &HeaderMap, hosts: &[String]) -> Response {
    // The hosts are the request's own text: shortened, and escaped, before they are printed.
    let named: Vec<String> = hosts.iter().map(|host| host.chars().take(100).flat_map(char::escape_default).collect()).collect();
    eprintln!("deltabadger: blocked hosts: {}", named.join(", "));
    let xhr = header_text(headers, "x-requested-with").is_some_and(|with| with.to_ascii_lowercase().contains("xmlhttprequest"));
    (StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, if xhr { "text/plain; charset=UTF-8" } else { "text/html; charset=UTF-8" })], "").into_response()
}

/// The largest form body `entry` reads, and the most fields it may hold. A form is read before any
/// route, session or rate limit sees the request, so the bound comes first. The largest form of the
/// pages served so far is the login form: a token of 86 characters, an email, a password of at
/// most 128 and three short fields, under 1 KiB. (`/csp-report` is not a form and its body is never read.)
pub const FORM_LIMIT: usize = 64 * 1024;
pub const FORM_FIELDS: usize = 1000;
/// The same bound for the query string, which is parsed for every request: 8 KiB (the longest path
/// kept as `return_to` is 2 KiB) and as many fields as a form.
pub const QUERY_LIMIT: usize = 8 * 1024;
pub const QUERY_FIELDS: usize = FORM_FIELDS;
/// The paths whose POST may carry `application/json` instead of a form: what an OAuth client sends
/// on its own (web::oauth). The body is read under the form's limit and deadline.
pub const JSON_PATHS: [&str; 3] = ["/oauth/register", "/oauth/token", "/oauth/revoke"];
/// How long the whole of a form's body may take to arrive.
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything that has to happen before a route is chosen:
/// - a request for a host this deployment does not allow is refused, before anything reads it;
/// - a static file is answered at once, as Rails' static file server sits in front of the app;
/// - form bodies on POST/PATCH/PUT/DELETE are parsed under one byte/field/deadline bound;
/// - only an original POST applies `_method`, as Rack::MethodOverride does;
/// - the path is normalised and its locale prefix taken off, so one set of routes serves `/login`
///   and `/de/login`.
async fn entry(State(entry): State<Entry>, request: Request) -> Response {
    let (mut parts, body) = request.into_parts();
    let blocked = entry.app.config.blocked_hosts(&parts.headers);
    if !blocked.is_empty() {
        return blocked_host(&parts.headers, &blocked);
    }
    let raw_query = parts.uri.query().unwrap_or("");
    if raw_query.len() > QUERY_LIMIT {
        return (StatusCode::URI_TOO_LONG, "Query string too long\n").into_response();
    }
    if form_urlencoded::parse(raw_query.as_bytes()).nth(QUERY_FIELDS).is_some() {
        return (StatusCode::BAD_REQUEST, "Too many query fields\n").into_response();
    }
    let full_path = normalize_path(parts.uri.path());
    if matches!(parts.method, Method::GET | Method::HEAD) {
        if let Some(file) = assets::find(&full_path) {
            return assets::respond(file, parts.method == Method::HEAD);
        }
    }
    let query = parts.uri.query().map(|q| pairs(q.as_bytes())).unwrap_or_default();
    let original_method = parts.method.clone();
    let (path_locale, route_path) = locale::split(&full_path);
    let route_path = route_path.to_string();
    let bot_json = bot::action_params::action(&route_path, &original_method).is_some();
    let content_type = header_text(&parts.headers, "content-type").unwrap_or("");
    let form_post = matches!(original_method, Method::POST | Method::PATCH | Method::PUT | Method::DELETE)
        && content_type.split(';').next().is_some_and(|v| v.trim().eq_ignore_ascii_case("application/x-www-form-urlencoded"));
    let json_post = (bot_json || (original_method == Method::POST && JSON_PATHS.contains(&full_path.as_str())))
        && content_type.split(';').next().is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    let mut json = None;
    let (form, body) = if form_post || json_post {
        // The one place a request body is read. It has a deadline for the whole of it: a client that
        // declares a body and stops sending gets a 408, and `connection: close` makes hyper drop the
        // connection, so it does not keep its place. (A body no handler reads is not waited for: hyper
        // closes the connection after the response.)
        let read = match tokio::time::timeout(entry.body_read_timeout, axum::body::to_bytes(body, FORM_LIMIT)).await {
            Ok(read) => read,
            Err(_) => return (StatusCode::REQUEST_TIMEOUT, [(header::CONNECTION, "close")], "The form did not arrive in time\n").into_response(),
        };
        match read {
            // A JSON body that is not JSON is Rails' 400, answered before any controller. One that is not
            // an object names no parameter, and neither does an empty one.
            Ok(bytes) if json_post && bot_json => match bot::action_params::json(&bytes) {
                Ok(value) => { json = Some(value); (Vec::new(), Body::empty()) }
                Err(_) => return (StatusCode::BAD_REQUEST, "Bad Request\n").into_response(),
            },
            Ok(bytes) if json_post => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => { json = value.is_object().then_some(value); (Vec::new(), Body::empty()) }
                Err(_) if bytes.iter().all(u8::is_ascii_whitespace) => (Vec::new(), Body::empty()),
                Err(_) => return (StatusCode::BAD_REQUEST, "Bad Request\n").into_response(),
            },
            Ok(bytes) if form_urlencoded::parse(&bytes).nth(FORM_FIELDS).is_some() => return (StatusCode::BAD_REQUEST, "Too many form fields\n").into_response(),
            Ok(bytes) => (pairs(&bytes), Body::empty()),
            Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Form too large\n").into_response(),
        }
    } else {
        (Vec::new(), body)
    };
    if original_method == Method::POST {
        if let Some(method) = method_override(&form) { parts.method = method; }
    }
    let with_query = |path: &str| parts.uri.query().map_or_else(|| path.to_string(), |q| format!("{path}?{q}"));
    let fullpath = with_query(&full_path);
    let Ok(uri) = with_query(&route_path).parse::<Uri>() else { return (StatusCode::BAD_REQUEST, "Bad Request\n").into_response() };
    parts.uri = uri;
    let params = Params { full_path, fullpath, route_path, path_locale, query, form, json };
    if bot::action_params::action(&params.route_path, &parts.method).is_some()
        && bot::action_params::ActionParams::parse(&params).is_err() {
        return (StatusCode::BAD_REQUEST, "Bad Request\n").into_response();
    }
    parts.extensions.insert(Arc::new(params));
    match entry.routes.oneshot(Request::from_parts(parts, body)).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

/// The whole web application as one service. `serve` binds it; tests call it with `oneshot`.
pub fn router(app: App) -> Router {
    router_with(app, BODY_READ_TIMEOUT)
}

/// `router`, with another deadline for a form's body (`server::Limits`).
pub(crate) fn router_with(app: App, body_read_timeout: Duration) -> Router {
    router_with_routes(app.clone(), routes(app), body_read_timeout)
}

/// Compose the bounded transport with a router (also permits transport probes without enabling actions).
pub fn router_with_routes(app: App, routes: Router, body_read_timeout: Duration) -> Router {
    Router::new().fallback(entry).with_state(Entry { app, routes, body_read_timeout })
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
    } else if bot::action_params::action(&context.params.route_path, request.method())
        .is_some_and(|action| !bot::action_params::format_allowed(action, &context.params.route_path, request.headers())) {
        StatusCode::NOT_ACCEPTABLE.into_response()
    } else {
        request.extensions_mut().insert(context);
        next.run(request).await
    };
    finish(&app, &session, &before, &nonce, now, signed_in, response)
}
