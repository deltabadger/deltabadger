//! Driving the web app in-process: a settable clock, an app on a scratch install, and a browser that
//! keeps its session cookie and the last page it loaded, and submits forms with the token that form carries.
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request};
use chrono::{DateTime, Utc};
use deltabadger::engine::Clock;
use deltabadger::store::{self, Paths};
use deltabadger::web::{self, App, Config};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

pub const SECRET: &str = "web-test-secret-key-base";
pub const HOST: &str = "localhost:3000";
const COOKIE: &str = "_deltabadger_rust_session";

pub struct TestClock(Mutex<DateTime<Utc>>);
impl TestClock {
    pub fn at(time: &str) -> Arc<Self> { Arc::new(Self(Mutex::new(at(time)))) }
    pub fn set(&self, time: DateTime<Utc>) { *self.0.lock().unwrap() = time; }
}
impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> { *self.0.lock().unwrap() }
}

pub fn at(time: &str) -> DateTime<Utc> {
    time.parse().unwrap()
}

pub fn header_map(pairs: &[(&'static str, &str)]) -> axum::http::HeaderMap {
    pairs.iter().map(|(name, value)| (axum::http::HeaderName::from_static(name), axum::http::HeaderValue::from_str(value).unwrap())).collect()
}

pub fn env(secret: &str) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| (name == "SECRET_KEY_BASE").then(|| secret.to_string())
}

/// The app on the install in `dir`, as `deltabadger serve` builds it, with the test's clock.
pub fn app(dir: &Path, secret: &str, clock: Arc<TestClock>) -> App {
    app_allowing(dir, secret, None, clock)
}

/// `app` on a deployment whose ALLOWED_HOSTS is `allowed_hosts`.
pub fn app_allowing(dir: &Path, secret: &str, allowed_hosts: Option<&str>, clock: Arc<TestClock>) -> App {
    let opened = store::open(&Paths::from_env(&|_| None, dir)).expect("the install passes store::check");
    let env = |name: &str| match name {
        "SECRET_KEY_BASE" => Some(secret.to_string()),
        "ALLOWED_HOSTS" => allowed_hosts.map(str::to_string),
        _ => None,
    };
    App::new(Config::from_env(&env).unwrap(), &env, opened.primary, clock).unwrap()
}

pub struct Answer {
    pub status: u16,
    /// Lower-case names; a repeated header keeps every value.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// One browser: its session cookie and the last page it loaded.
#[derive(Default)]
pub struct Browser {
    pub cookie: Option<String>,
    /// The body of the last response that carried a CSRF meta tag.
    pub page: Option<String>,
}

/// Where a request carries a CSRF token, as a real browser would:
/// - `Form`: the hidden `authenticity_token` of the form on the last page whose `action` is this path;
/// - `Header`: the page's `<meta name="csrf-token">` in `X-CSRF-Token`, as the compiled JS's `fetch` calls send it;
/// - `Both`: a Turbo form submission, which sends the form's field and the header;
/// - `None`: nowhere.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Csrf { Form, Header, Both, None }

/// The `content` of the CSRF meta tag of a page.
pub fn meta_token(page: &str) -> Option<String> {
    page.split("<meta name=\"csrf-token\" content=\"").nth(1).and_then(|rest| rest.split('"').next()).map(str::to_string)
}

/// The hidden `authenticity_token` of the first form on `page` that posts to `action`.
pub fn form_token(page: &str, action: &str) -> Option<String> {
    page.split("<form").skip(1).find_map(|form| {
        let (tag, rest) = form.split_once('>')?;
        if !tag.contains(&format!(" action=\"{action}\"")) { return None; }
        let inner = rest.split("</form>").next()?;
        inner.split("name=\"authenticity_token\" value=\"").nth(1).and_then(|value| value.split('"').next()).map(str::to_string)
    })
}

impl Browser {
    pub async fn get(&mut self, app: &App, path: &str) -> Answer {
        self.send(app, "GET", path, None, Csrf::None, &[]).await
    }

    /// Submits the form the last page has for `path`.
    pub async fn post(&mut self, app: &App, path: &str, form: &[(&str, &str)]) -> Answer {
        self.send(app, "POST", path, Some(form), Csrf::Form, &[]).await
    }

    pub async fn send(&mut self, app: &App, method: &str, path: &str, form: Option<&[(&str, &str)]>, csrf: Csrf, headers: &[(&str, &str)]) -> Answer {
        let mut request = Request::builder().method(method).uri(path).header(header::HOST, HOST);
        if let Some(cookie) = &self.cookie {
            request = request.header(header::COOKIE, format!("{COOKIE}={cookie}"));
        }
        for (name, value) in headers { request = request.header(*name, *value); }
        let page = self.page.clone().unwrap_or_default();
        if matches!(csrf, Csrf::Header | Csrf::Both) {
            request = request.header("x-csrf-token", meta_token(&page).expect("the last page has a csrf-token meta tag"));
        }
        let body = match form {
            Some(fields) => {
                let mut encoded = form_urlencoded::Serializer::new(String::new());
                for (name, value) in fields { encoded.append_pair(name, value); }
                if matches!(csrf, Csrf::Form | Csrf::Both) {
                    let action = path.split('?').next().unwrap();
                    encoded.append_pair("authenticity_token", &form_token(&page, action).unwrap_or_else(|| panic!("the last page has no form posting to {action}")));
                }
                request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(encoded.finish())
            }
            None => Body::empty(),
        };
        let mut request = request.body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
        let response = web::router(app.clone()).oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response.headers().iter().map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap().to_string())).collect();
        let body = String::from_utf8_lossy(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).into_owned();
        for (_, value) in headers.iter().filter(|(name, _)| name == "set-cookie") {
            if let Some(rest) = value.strip_prefix(&format!("{COOKIE}=")) {
                self.cookie = Some(rest.split(';').next().unwrap().to_string());
            }
        }
        if meta_token(&body).is_some() {
            self.page = Some(body.clone());
        }
        Answer { status, headers, body }
    }
}

/// Writes `sent` on a new connection and reads until the server closes it. `Err` when it is still
/// open after `patience`.
pub async fn until_closed(address: std::net::SocketAddr, sent: &'static [u8], patience: std::time::Duration) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(address)?;
        stream.set_read_timeout(Some(patience))?;
        std::io::Write::write_all(&mut stream, sent)?;
        let mut answer = String::new();
        std::io::Read::read_to_string(&mut stream, &mut answer).map(|_| answer)
    }).await.unwrap()
}

/// Sends one GET on an open keep-alive connection; `false` when the write failed.
pub fn send_get(stream: &mut std::net::TcpStream, path: &str) -> bool {
    use std::io::Write;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").is_ok()
}

/// Reads one answer to the end of its body (by Content-Length); `None` when the server closed the connection instead.
pub fn read_answer(stream: &mut std::net::TcpStream) -> Option<String> {
    use std::io::Read;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 { return None; }
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).ok()?;
    let length = head.lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().ok()))
        .flatten().unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).ok()?;
    Some(head + &String::from_utf8_lossy(&body))
}

/// One GET on an open keep-alive connection; `None` when the server closed it instead of answering.
pub fn keep_alive_get(stream: &mut std::net::TcpStream, path: &str) -> Option<String> {
    if !send_get(stream, path) { return None; }
    read_answer(stream)
}
