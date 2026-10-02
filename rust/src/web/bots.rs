//! `home#index`, and `bots#index` as far as this plan serves it.
use super::auth;
use super::layout::{self, Ctx};
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::Response;

/// GET /: to the bots page when signed in, else to the login page. Both keep the request's locale.
pub async fn home(Extension(ctx): Extension<Ctx>) -> Response {
    layout::redirect(StatusCode::FOUND, &ctx.path(if ctx.user().is_some() { "/bots" } else { "/login" }))
}

/// GET /bots: for a signed-in user only. The page itself arrives in Task 16.
pub async fn index(Extension(ctx): Extension<Ctx>) -> Response {
    if ctx.user().is_none() {
        return auth::unauthenticated(&ctx);
    }
    layout::not_ported_response(&ctx.method, &ctx.params.fullpath, ctx.turbo_frame.as_deref())
}
