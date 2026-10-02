//! The engine, the web UI and the background services in one process, on one runtime, under one lock.
//! `deltabadger serve` passes the web; `deltabadger run` passes none, so both
//! commands go through this one supervisor and its one rule. Nothing here is spawned: the engine future is not `Send`,
//! and the services need not be. One stop signal, the engine's `Shutdown`. One "ended" rule: the first part to end for
//! any reason other than a requested stop decides `Ended` and requests the stop; the web stops serving when the engine
//! returns (no half-alive process serves pages while nothing trades); `serve` returns once the engine and every service
//! have finished their unit in hand (a tick is never cut at an await point). Connections already accepted end with the
//! runtime (`main.rs`).
use crate::engine::run::{self, Engine, Shutdown};
use crate::engine::{log, Clock, EngineError};
use crate::venue::VenueFactory;
use crate::web::server::{serve_on, Limits};
use crate::web::{App, WebError};
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use tokio::net::TcpListener;

#[derive(Debug)]
pub enum Ended {
    /// A stop request (SIGTERM/SIGINT): every part finished its unit in hand, and no service failed.
    Stopped,
    /// The engine failed; the web stopped with it and the services drained.
    Engine(EngineError),
    /// The web server failed (never without a web); the engine and the services were asked to stop and finished their unit first.
    Web(WebError),
    /// A service ended unasked (an error, or a return before any stop); the others drained.
    Service { name: &'static str, error: String },
}

/// A background service of `serve`: a scheduler, the ledger and balance sync, the mail sender. Built in
/// `main.rs` after the claim, with the one stop signal (`engine.stop_handle()`); once a stop is requested it returns
/// `Ok(())` promptly. `Err`, or `Ok` before a stop was requested, ends the process. Polled on the runtime thread, never
/// spawned, on its own connection; no stretch between two awaits may hold the thread longer than the engine may.
pub struct Service<'a> {
    pub name: &'static str,
    pub run: Pin<Box<dyn Future<Output = Result<(), String>> + 'a>>,
}

/// Runs `engine`, serves `web` (the app and its bound listener; `None` for `run`) and runs `services` until they end as
/// the module doc says. The caller has claimed the install and registered the stop signals (`Shutdown::on_signals`).
pub async fn serve<'a, F: VenueFactory>(mut engine: Engine<F>, web: Option<(App, TcpListener)>, clock: &'a dyn Clock, mut services: Vec<Service<'a>>) -> Ended {
    // The install stays locked until every part has finished: the engine drops its own handle when it returns, before
    // the services drain (`main.rs` holds one more through the runtime's shutdown).
    let _lock = engine.lock.clone();
    // Armed whenever anything beside the engine may write in this process. Bare `run` (no web, no service) keeps
    // the merged path: an ineligible pass is `Err(Ineligible)`.
    engine.writers_guarded = web.is_some() || !services.is_empty();
    if let Some((app, _)) = &web { app.attach_engine(engine.wake_handle()); }
    let stop = engine.stop_handle();
    let mut engine_run = Box::pin(run::run(engine, clock));
    // Without a web this part never ends, so `run` and `serve` share the loop below.
    let mut web_run = Box::pin(async move {
        match web {
            Some((app, listener)) => serve_on(listener, app, Limits::default()).await,
            None => std::future::pending().await,
        }
    });
    let mut web_open = true;
    let mut first: Option<Ended> = None; // the first part that ended unasked, or the first service that failed
    let engine_result = loop {
        tokio::select! {
            r = &mut engine_run => break r,
            r = &mut web_run, if web_open => {
                web_open = false;
                first.get_or_insert(Ended::Web(r.err().unwrap_or_else(|| WebError::Config("the web server stopped".into()))));
                stop.request();
            }
            (name, r) = next_end(&mut services) => {
                if let Some(error) = service_failed(name, r, &stop) {
                    first.get_or_insert(Ended::Service { name, error });
                    stop.request();
                }
            }
        }
    };
    // The engine's own failure counts before anything that fails while the services drain: the first failure decides.
    first = match (first, engine_result) {
        (Some(ended), Err(e)) => {
            if !matches!(e, EngineError::Stopped) { log(&format!("[engine] stopped after {ended:?}: {e:?}")); }
            Some(ended)
        }
        (None, Err(EngineError::Stopped)) => None,
        (None, Err(e)) => Some(Ended::Engine(e)),
        (_, Ok(never)) => match never {},
    };
    drop(web_run); // nothing serves once the engine has returned
    stop.request(); // an engine that failed on its own: the services stop too
    while !services.is_empty() {
        let (name, r) = next_end(&mut services).await;
        // A later failure is logged by `service_failed`; it decides only if nothing ended the process before it.
        if let Some(error) = service_failed(name, r, &stop) { first.get_or_insert(Ended::Service { name, error }); }
    }
    first.unwrap_or(Ended::Stopped)
}

/// Whether a service's end decides how the process ends: an error at any time, a requested stop's drain included, or
/// a return before any stop was requested (a service with nothing to do waits; it does not return).
fn service_failed(name: &str, r: Result<(), String>, stop: &Shutdown) -> Option<String> {
    match r {
        Err(error) => {
            log(&format!("[{name}] ended: {error}"));
            Some(error)
        }
        Ok(()) if !stop.is_requested() => Some("returned before a stop was requested".into()),
        Ok(()) => None,
    }
}

/// The next service to end, taken out of the set. Pending while none has; forever when the set is empty.
/// ponytail: every wake polls every service; fine for a handful, a LocalSet + JoinSet if they ever number dozens.
async fn next_end(services: &mut Vec<Service<'_>>) -> (&'static str, Result<(), String>) {
    std::future::poll_fn(|cx| {
        let mut done = None;
        for (i, s) in services.iter_mut().enumerate() {
            if let Poll::Ready(r) = s.run.as_mut().poll(cx) { done = Some((i, r)); break; }
        }
        match done {
            Some((i, r)) => Poll::Ready((services.swap_remove(i).name, r)),
            None => Poll::Pending,
        }
    }).await
}
