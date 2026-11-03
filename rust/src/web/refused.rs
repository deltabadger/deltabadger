//! The refusal page (PLAN-B2 r1.0, r2.8): when `serve` refuses an install, it still opens. It serves one static
//! page with the refusal for every path until SIGTERM/SIGINT, so a refusal on Docker, Umbrel or the desktop is seen in
//! the browser and not only in a log. It holds no database connection and no engine lock, and writes nothing.
//! `/health-check` and `/up` answer 503 (a refused instance is never "deployed"); `/ui-ready` answers 200, as in the
//! normal app, so the desktop knows something is served.
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::net::SocketAddr;

/// The page for `reason` (the refusal as `serve` printed it).
pub fn page(reason: &str) -> String {
    document("Deltabadger didn't start", "Deltabadger didn't start trading on this data.",
             &format!("<pre style=\"white-space: pre-wrap; background: #f4f4f4; padding: 1rem\">{}</pre>", super::i18n::escape(reason)), "")
}

/// The page while `serve` waits `wait` seconds for a just-stopped 2.x to be gone (PLAN-B2 r2.8). It reloads itself, so the
/// app appears once the wait is over.
pub fn starting_page(wait: u64) -> String {
    document("Deltabadger is starting", "Deltabadger is starting",
             &format!("<p>The previous version stopped a moment ago. Deltabadger is waiting about {wait} seconds to make sure it has \
                       fully shut down, then your bots will start again. This page reloads on its own.</p>"),
             "<meta http-equiv=\"refresh\" content=\"5\">")
}

fn document(title: &str, heading: &str, body: &str, head: &str) -> String {
    format!("<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             {head}<title>{title}</title></head>\
             <body style=\"font-family: system-ui, sans-serif; max-width: 48rem; margin: 3rem auto; padding: 0 1rem; line-height: 1.5\">\
             <h1 style=\"font-size: 1.4rem\">{heading}</h1>{body}</body></html>")
}

/// The page's routes: `/ui-ready` 200, `/health-check` and `/up` 503, anything else the page with 503.
pub fn router(reason: &str) -> Router { routes(page(reason), "refused") }

fn routes(page: String, state: &'static str) -> Router {
    let body = std::sync::Arc::new(page);
    let refused = move || {
        let body = body.clone();
        async move {
            (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")],
             body.as_str().to_owned()).into_response()
        }
    };
    let unhealthy = || async { (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], r#"{"health":"refused"}"#).into_response() };
    Router::new()
        .route("/ui-ready", get(move || ui_ready(state)))
        .route("/health-check", get(unhealthy))
        .route("/up", get(unhealthy))
        .fallback(refused)
}

static READY_TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `serve`'s `DELTABADGER_READY_TOKEN`, set once at start. The desktop app mints one per start and trusts a `/ui-ready`
/// only if it carries it, so another program on the port is never taken for its engine (PLAN-B6 r1.4).
pub fn set_ready_token(token: Option<String>) {
    if let Some(t) = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) { let _ = READY_TOKEN.set(t); }
}

/// `/ui-ready`: 200 whenever something is served to the user, the app or this page (PLAN-B2 r2.8). With a ready token,
/// `ready <token> <state>`, `state` being `app`, `refused` or `starting`; without, `ready`.
pub async fn ui_ready(state: &'static str) -> Response {
    let body = match READY_TOKEN.get() { Some(t) => format!("ready {t} {state}"), None => "ready".into() };
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

/// Serves the page on `port` at the bind address (server::binding) until SIGTERM/SIGINT. 0 = stopped as asked; 1 = the port could not be taken
/// (the refusal stands; nothing else to show it on).
pub fn serve(port: u16, reason: &str) -> i32 {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    rt.block_on(async {
        let listener = match tokio::net::TcpListener::bind(SocketAddr::from((super::server::binding(), port))).await {
            Ok(l) => l,
            Err(e) => { crate::eline!("deltabadger: cannot show the refusal page on port {port}: {e}"); return 1; }
        };
        let stop = stop_signal(); // registered before the line below, so a stop sent after it is never missed
        crate::engine::log(&format!("refused: showing why on port {port} until stopped"));
        // After the stop signal, open connections get 5 s; a client holding a half-sent request never keeps it longer.
        let signalled = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify = signalled.clone();
        let served = axum::serve(listener, router(reason)).with_graceful_shutdown(async move { stop.await; notify.notify_one(); });
        tokio::select! {
            result = served => match result { Ok(()) => 0, Err(e) => { crate::eline!("deltabadger: the refusal page stopped: {e}"); 1 } },
            _ = async { signalled.notified().await; tokio::time::sleep(std::time::Duration::from_secs(5)).await } => 0,
        }
    })
}

/// `starting_page` on `port` at the bind address (server::binding) for as long as the future is polled: `serve` waiting out a Rails heartbeat (PLAN-B2
/// r2.8), with the refusal page's routes. Dropping it closes the listener; the runtime's end closes the connections still
/// open. Returns only when the port cannot be taken.
pub async fn show_starting(port: u16, wait: u64) -> std::io::Error {
    let listener = match tokio::net::TcpListener::bind(SocketAddr::from((super::server::binding(), port))).await { Ok(l) => l, Err(e) => return e };
    match axum::serve(listener, routes(starting_page(wait), "starting")).await { Ok(()) => std::io::Error::other("the page stopped"), Err(e) => e }
}

/// SIGTERM or SIGINT (Ctrl-C off Unix), with the handlers registered now, not on the first poll; or the desktop's stdin
/// latch (crate::stop).
#[cfg(unix)]
pub fn stop_signal() -> impl std::future::Future<Output = ()> {
    use tokio::signal::unix::{signal, SignalKind};
    let (mut term, mut int) = (signal(SignalKind::terminate()).expect("a SIGTERM handler"), signal(SignalKind::interrupt()).expect("a SIGINT handler"));
    async move { tokio::select! { _ = term.recv() => {}, _ = int.recv() => {}, _ = crate::stop::latched() => {} } }
}

#[cfg(not(unix))]
pub fn stop_signal() -> impl std::future::Future<Output = ()> {
    let mut ctrl_c = tokio::signal::windows::ctrl_c().expect("a Ctrl-C handler");
    async move { tokio::select! { _ = ctrl_c.recv() => {}, _ = crate::stop::latched() => {} } }
}
