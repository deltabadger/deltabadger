//! 3.0 A26: the legacy routes Rails still draws, answered for whoever can still send them after the upgrade (a bookmark,
//! an outside link). Not a port: the cheapest answer that leaves the caller somewhere sensible (PLAN-A26-legacy-routes.md).
use super::assets;
use super::layout::{self, Ctx};
use axum::extract::Extension;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

/// GET /dashboard: Rails' 301 to the bots page, written as Rust writes its paths (D-LEGACY-DASHBOARD-PATH).
pub async fn dashboard(Extension(ctx): Extension<Ctx>) -> Response {
    layout::redirect(StatusCode::MOVED_PERMANENTLY, &ctx.path("/bots"))
}

/// GET /cryptocurrency-dollar-cost-averaging: a marketing page neither app has. Rails 301s to a locale copy that is a 404,
/// which its exceptions_app sends to the root; this goes there in one hop (D-LEGACY-DCA-PAGE-ONE-HOP).
pub async fn dca_page(Extension(ctx): Extension<Ctx>) -> Response {
    layout::redirect(StatusCode::MOVED_PERMANENTLY, &ctx.path("/"))
}

/// ErrorsController#redirect_to_root, any method.
pub async fn not_found(Extension(ctx): Extension<Ctx>) -> Response {
    layout::redirect(StatusCode::FOUND, &ctx.path("/"))
}

/// ErrorsController#unprocessable_entity, any method.
pub async fn unprocessable() -> Response { public(StatusCode::UNPROCESSABLE_ENTITY, "/422.html") }

/// ErrorsController#internal_server_error, any method.
pub async fn server_error() -> Response { public(StatusCode::INTERNAL_SERVER_ERROR, "/500.html") }

/// A request no 2.x page sends any more: `POST /bots/:id/show` (D-LEGACY-SHOW-POST-ABSENT), `POST /api/remove_invalid_keys`
/// (D-LEGACY-REMOVE-INVALID-KEYS-ABSENT), and `POST /bots`, `DELETE /bots/:id`, which `resources :bots` draws for actions
/// BotsController never had (D-LEGACY-BOTS-CREATE-DESTROY-ABSENT; Rails' 404 ends at the root).
pub async fn absent() -> Response { layout::missing() }

fn public(status: StatusCode, file: &str) -> Response {
    let body = assets::find(file).map_or(&b""[..], |file| file.body);
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}
