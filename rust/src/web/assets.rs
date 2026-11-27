//! The files `serve` hands out, embedded at build time (build.rs `assets`): the bun and dart-sass
//! builds and rust/web/images at fingerprinted /assets/ paths, and rust/web/public/ at its own paths.
//! They are answered before the session and CSRF code runs, as 2.x's static file server does.
use axum::body::Body;
use axum::http::{header, HeaderValue, Response, StatusCode};

pub struct Embedded {
    /// The path this file is served at, e.g. `/assets/application-2ec3a7c0cf99c409.js` or `/fonts/x.woff2`.
    pub url: &'static str,
    /// The name views ask for, e.g. `application.js` or `favicon/favicon.svg`; empty for rust/web/public/ files.
    pub logical: &'static str,
    pub content_type: &'static str,
    pub body: &'static [u8],
}

// `EMBEDDED`, and `BUILT`: whether this build has the web assets at all (build.rs `assets`).
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// What a build without the web assets says instead of serving pages with no script and no style.
pub const MISSING: &str = "built without web assets; run script/rust/build_assets.sh and build again";

/// `Err` with that sentence in a build that has no web assets.
pub fn require() -> Result<(), super::WebError> {
    if BUILT { Ok(()) } else { Err(super::WebError::Config(MISSING.into())) }
}

/// The built files a dev server reads from disk instead (`dev`).
const DEV_FILES: [(&str, &str); 3] = [
    ("application.css", "text/css; charset=utf-8"),
    ("application.js", "text/javascript; charset=utf-8"),
    ("application.js.map", "application/json"),
];

/// Debug builds only: `DELTABADGER_DEV_ASSETS` names the folder the watchers write (rust/web/builds, bin/rust-dev).
/// Its built files are then read on every request at plain /assets/<name>, never cached, so a saved stylesheet shows
/// on the next reload. A release build has no such path: it serves only what it embedded.
#[cfg(debug_assertions)]
fn dev_dir() -> Option<&'static std::path::Path> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("DELTABADGER_DEV_ASSETS").filter(|d| !d.is_empty()).map(Into::into)).as_deref()
}
#[cfg(not(debug_assertions))]
fn dev_dir() -> Option<&'static std::path::Path> {
    None
}

/// The response for `url` when it is one of the dev server's files (`dev_dir`); `None` otherwise.
pub fn dev(url: &str, head: bool) -> Option<Response<Body>> {
    dev_file(dev_dir()?, url, head)
}

fn dev_file(dir: &std::path::Path, url: &str, head: bool) -> Option<Response<Body>> {
    let name = url.strip_prefix("/assets/")?;
    let (name, content_type) = DEV_FILES.into_iter().find(|(file, _)| *file == name)?;
    let (status, body) = match std::fs::read(dir.join(name)) {
        Ok(bytes) => (StatusCode::OK, bytes),
        Err(error) => (StatusCode::NOT_FOUND, format!("{}: {error}", dir.join(name).display()).into_bytes()),
    };
    let length = body.len();
    let mut response = Response::new(if head { Body::empty() } else { Body::from(body) });
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    Some(response)
}

pub fn find(url: &str) -> Option<&'static Embedded> {
    EMBEDDED.binary_search_by(|file| file.url.cmp(url)).ok().map(|i| &EMBEDDED[i])
}

/// 2.x's `asset_path`: the fingerprinted path of a logical name. An unknown name answers with a
/// path nothing is served at, so the mistake shows as one missing file; tests/web.rs checks every
/// name the templates use.
pub fn path(logical: &str) -> &'static str {
    if dev_dir().is_some() {
        match logical {
            "application.css" => return "/assets/application.css",
            "application.js" => return "/assets/application.js",
            _ => {}
        }
    }
    EMBEDDED.iter().find(|file| file.logical == logical).map_or("/assets/missing", |file| file.url)
}

/// 2.x's `public_file_server.headers`: one year for every static file.
pub fn respond(file: &'static Embedded, head: bool) -> Response<Body> {
    let mut response = Response::new(if head { Body::empty() } else { Body::from(file.body) });
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(file.content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(file.body.len()));
    response
}

/// /service-worker.js on every platform (PLAN-B6 r4.1, D-DESKTOP-SW-CACHE). The old worker cached `/` at install and
/// answered from that cache first: a WebView could show an old app page while a start was refused, and since `/` is a
/// 302, a browser got the cached redirect for the page load and showed ERR_FAILED until a hard refresh. This worker
/// replaces it at the next update check, deletes every cache, unregisters itself, and reloads each page it took over,
/// uncontrolled. It has no fetch handler: every request goes to the network. Nothing registers a worker any more.
pub const RETIRING_WORKER: &str = "\
self.addEventListener('install', function () { self.skipWaiting(); });
self.addEventListener('activate', function (event) {
  event.waitUntil(
    caches.keys()
      .then(function (keys) { return Promise.all(keys.map(function (key) { return caches.delete(key); })); })
      .then(function () { return self.registration.unregister(); })
      .then(function () { return self.clients.matchAll(); })
      .then(function (clients) { clients.forEach(function (client) { client.navigate(client.url); }); })
  );
});
";

/// `RETIRING_WORKER`, never cached, so the WebView's next update check always sees it.
pub fn retiring_worker(head: bool) -> Response<Body> {
    let mut response = Response::new(if head { Body::empty() } else { Body::from(RETIRING_WORKER) });
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/javascript; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(RETIRING_WORKER.len()));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_files_are_read_from_disk_on_every_request_and_never_cached() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("application.css"), "a{}").unwrap();
        let response = dev_file(dir.path(), "/assets/application.css", false).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "3");
        std::fs::write(dir.path().join("application.css"), "b{color:red}").unwrap();
        assert_eq!(dev_file(dir.path(), "/assets/application.css", true).unwrap().headers()[header::CONTENT_LENGTH], "12");
        // Not yet written by the watcher: a 404 that names the file.
        assert_eq!(dev_file(dir.path(), "/assets/application.js", false).unwrap().status(), StatusCode::NOT_FOUND);
        // Only the built files: nothing else in the folder, and no way out of it.
        assert!(dev_file(dir.path(), "/assets/../Cargo.toml", false).is_none());
        assert!(dev_file(dir.path(), "/assets/other.css", false).is_none());
        assert!(dev_file(dir.path(), "/application.css", false).is_none());
    }
}
