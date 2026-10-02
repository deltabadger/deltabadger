//! `deltabadger serve`: the web server on its own. The engine loop joins this process in a later plan.
use super::{router, App, WebError};
use std::net::SocketAddr;

/// Binds 0.0.0.0:`port` and serves until the process is stopped. Refuses an install with no admin
/// user: until 3.0 Rails creates the install, and setup is a Rails page.
pub async fn serve(app: App, port: u16) -> Result<(), WebError> {
    let admins: i64 = app.db(|c| Ok(c.query_row("SELECT count(*) FROM users WHERE admin = 1", [], |r| r.get(0))?)).await?;
    if admins == 0 {
        return Err(WebError::Config("this install has no admin user yet: set it up with the Rails app first".into()));
    }
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await
        .map_err(|e| WebError::Config(format!("cannot listen on port {port}: {e}")))?;
    eprintln!("deltabadger: serving on port {port}");
    axum::serve(listener, router(app).into_make_service_with_connect_info::<SocketAddr>()).await
        .map_err(|e| WebError::Config(format!("the server stopped: {e}")))
}
