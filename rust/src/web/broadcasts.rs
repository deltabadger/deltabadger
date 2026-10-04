//! BroadcastsController, for the three figures a page waits on: `broadcast--on-connect` posts here once every stream
//! source of the page is connected (app/javascript/controllers/broadcast/on_connect_controller.js). Rails answers
//! `head :ok` at once and leaves the broadcast to a job; so does this build, and the work is `figure::loading::publish`.
//! The other methods of the controller (open orders, the limit rules' info, the wake) are not served yet.
use super::layout::Ctx;
use super::{auth, bot, App, WebError};
use axum::extract::{Extension, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// `head status`: no body, and the type Rails gives an answer without one.
fn head(status: StatusCode) -> Response {
    (status, [(header::CONTENT_TYPE, "text/html")]).into_response()
}

/// `params[name]` as `find_by(id:)` reads it: a JSON number, or text read as the bot of a path is read.
fn id(ctx: &Ctx, name: &str) -> Option<i64> {
    match ctx.params.json.as_ref().and_then(|json| json.get(name)) {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => bot::id_from_path(text),
        Some(_) => None,
        None => ctx.params.query(name).or_else(|| ctx.params.form(name)).and_then(bot::id_from_path),
    }
}

/// As Rails' job does, the work never changes the answer.
async fn publish(app: &App, user: i64) {
    let _ = super::figure::loading::publish(app, user).await;
}

/// `metrics_update`: the bot page's metrics and chart. A bot that is not the user's is "not found"; a deleted one is found.
pub async fn metrics_update(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = id(&ctx, "bot_id") else { return Ok(head(StatusCode::NOT_FOUND)) };
    let owned = app.db(move |c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id = ?1 AND user_id = ?2)", [id, user], |r| r.get::<_, bool>(0))?)).await?;
    if !owned { return Ok(head(StatusCode::NOT_FOUND)); }
    publish(&app, user).await;
    Ok(head(StatusCode::OK))
}

/// `pnl_update`: the tiles of `/bots`.
pub async fn pnl_update(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    publish(&app, user).await;
    Ok(head(StatusCode::OK))
}

/// `global_pnl_update`: the account headline.
pub async fn global_pnl_update(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    publish(&app, user).await;
    Ok(head(StatusCode::OK))
}
