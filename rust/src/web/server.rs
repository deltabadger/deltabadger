//! `deltabadger serve`: the web server on its own. The engine loop joins this process in a later plan.
use super::{router_with, App, WebError};
use axum::body::Body;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower::ServiceExt;

/// What the server allows a client that has not yet been routed.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long a client may take to send a request's head.
    pub header_read_timeout: Duration,
    /// How long a client may take to send a form's body, once its head is in.
    pub body_read_timeout: Duration,
    /// Connections open at once, WebSockets included.
    pub max_connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { header_read_timeout: Duration::from_secs(10), body_read_timeout: super::BODY_READ_TIMEOUT, max_connections: 1024 }
    }
}

/// Binds 0.0.0.0:`port` and serves until the process is stopped. Refuses an install with no admin
/// user: until 3.0 Rails creates the install, and setup is a Rails page.
pub async fn serve(app: App, port: u16) -> Result<(), WebError> {
    let admins: i64 = app.db(|c| Ok(c.query_row("SELECT count(*) FROM users WHERE admin = 1", [], |r| r.get(0))?)).await?;
    if admins == 0 {
        return Err(WebError::Config("this install has no admin user yet: set it up with the Rails app first".into()));
    }
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await
        .map_err(|e| WebError::Config(format!("cannot listen on port {port}: {e}")))?;
    eprintln!("deltabadger: serving on port {port}");
    serve_on(listener, app, Limits::default()).await
}

/// A connection that holds one of the server's places for as long as it is open. The place is part
/// of the stream, so it also stays taken after an upgrade: a WebSocket on /cable is still a connection.
struct Counted {
    stream: TcpStream,
    _place: OwnedSemaphorePermit,
}

impl AsyncRead for Counted {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Counted {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

/// Serves `app` on `listener` until the process is stopped. This is hyper's HTTP/1 connection, run
/// here instead of through `axum::serve`, which offers neither of the two limits:
/// - a client has `header_read_timeout` to send a request's head, or the connection is closed
///   (hyper's `http1::Builder::header_read_timeout`). There is no deadline on a whole request: a
///   WebSocket on /cable stays open for as long as its page does;
/// - at most `max_connections` connections are open. One more is not accepted until a place is
///   free: it waits in the listener's backlog.
///
/// Every request carries its peer's address (`ConnectInfo`), which the rate limiter keys on.
pub async fn serve_on(listener: TcpListener, app: App, limits: Limits) -> Result<(), WebError> {
    let router = router_with(app, limits.body_read_timeout);
    let places = Arc::new(Semaphore::new(limits.max_connections));
    loop {
        let place = places.clone().acquire_owned().await.map_err(|e| WebError::Config(format!("the server stopped: {e}")))?;
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            // Out of file descriptors, or the client was gone already: neither ends the server.
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let router = router.clone();
        let service = service_fn(move |mut request: hyper::Request<Incoming>| {
            request.extensions_mut().insert(ConnectInfo(peer));
            router.clone().oneshot(request.map(Body::new))
        });
        tokio::spawn(async move {
            let io = TokioIo::new(Counted { stream, _place: place });
            // An error here is one client's broken connection; there is nobody to tell.
            let _ = http1::Builder::new().timer(TokioTimer::new()).header_read_timeout(limits.header_read_timeout)
                .serve_connection(io, service).with_upgrades().await;
        });
    }
}
