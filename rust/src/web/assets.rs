//! The files `serve` hands out, embedded at build time (build.rs `assets`): the bun and dart-sass
//! builds and app/assets/images at fingerprinted /assets/ paths, and public/ at its own paths.
//! They are answered before the session and CSRF code runs, as Rails' static file server does.
use axum::body::Body;
use axum::http::{header, HeaderValue, Response, StatusCode};

pub struct Embedded {
    /// The path this file is served at, e.g. `/assets/application-2ec3a7c0cf99c409.js` or `/fonts/x.woff2`.
    pub url: &'static str,
    /// The name views ask for, e.g. `application.js` or `favicon/favicon.svg`; empty for public/ files.
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

pub fn find(url: &str) -> Option<&'static Embedded> {
    EMBEDDED.binary_search_by(|file| file.url.cmp(url)).ok().map(|i| &EMBEDDED[i])
}

/// Rails' `asset_path`: the fingerprinted path of a logical name. An unknown name answers with a
/// path nothing is served at, so the mistake shows as one missing file; tests/web.rs checks every
/// name the templates use.
pub fn path(logical: &str) -> &'static str {
    EMBEDDED.iter().find(|file| file.logical == logical).map_or("/assets/missing", |file| file.url)
}

/// config/environments/production.rb `public_file_server.headers`: one year for every static file.
pub fn respond(file: &'static Embedded, head: bool) -> Response<Body> {
    let mut response = Response::new(if head { Body::empty() } else { Body::from(file.body) });
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(file.content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(file.body.len()));
    response
}

/// The desktop's /service-worker.js (PLAN-B6 r4.1, D-DESKTOP-SW-CACHE): public/service-worker.js caches `/` and answers
/// from that cache first, so a WebView could show an old app page while this start is refused. This worker replaces it
/// at the next update check, deletes every cache, unregisters itself, and reloads each page it took over, uncontrolled.
/// It has no fetch handler: every request goes to the network. Browsers on Docker, Umbrel and hosted keep public/'s.
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
