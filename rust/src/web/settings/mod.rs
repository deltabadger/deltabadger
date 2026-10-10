//! Settings on the existing authenticated browser pipeline.
pub mod account;
pub mod confirmation;
pub mod api;
pub mod clients;
pub mod keys;
pub mod validator;
pub mod mail;
mod qr;
pub mod two_fa;
pub mod view;
use super::{
    auth,
    layout::{self, Ctx},
    App, WebError,
};
use axum::{
    extract::{Extension, State},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};

pub async fn root(Extension(ctx): Extension<Ctx>) -> Response {
    layout::redirect(StatusCode::FOUND, &ctx.path("/settings/connect"))
}
pub async fn show(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
) -> Result<Response, WebError> {
    if ctx.user().is_none() {
        return Ok(auth::unauthenticated(&ctx));
    }
    match ctx.params.route_path.as_str() {
        "/settings/edit_two_fa" => two_fa::handle(app, ctx, false).await,
        p if p.starts_with("/settings/confirm_revoke_mcp_client/") => clients::show(app, ctx).await,
        "/settings/api" => api::show(app, ctx).await,
        "/settings/connect" => keys::show(app, ctx).await,
        p if p.starts_with("/settings/confirm_destroy_api_key/")
            || p.starts_with("/settings/api_key_permissions/") =>
        {
            keys::modal(app, ctx).await
        }
        "/settings/account" => view::account(&app, &ctx, StatusCode::OK, None, vec![]).await,
        _ => Ok(layout::not_ported_response(
            &ctx.method,
            &ctx.params.fullpath,
            ctx.turbo_frame.as_deref(),
        )),
    }
}
pub async fn write(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if ctx.user().is_none() {
        return Ok(auth::unauthenticated(&ctx));
    }
    if (ctx.method == Method::PATCH
        && ctx
            .params
            .route_path
            .starts_with("/settings/update_client_tool_permissions/"))
        || (ctx.method == Method::DELETE
            && ctx
                .params
                .route_path
                .starts_with("/settings/revoke_mcp_client/"))
    {
        return clients::write(app, ctx).await;
    }
    if ctx.method == Method::PATCH
        && [
            "update_mcp_tool_permissions",
            "update_mcp_tool_group_permissions",
            "update_rest_tool_permissions",
            "update_rest_tool_group_permissions",
            "update_mcp_dry_run",
        ]
        .iter()
        .any(|s| ctx.params.route_path == format!("/settings/{s}"))
    {
        return api::write(app, ctx).await;
    }
    if ctx.method == Method::DELETE
        && ctx
            .params
            .route_path
            .starts_with("/settings/destroy_api_key/")
    {
        return keys::delete(app, ctx).await;
    }
    if ctx.method == Method::PATCH && ctx.params.route_path == "/settings/update_two_fa" {
        return two_fa::handle(app, ctx, true).await;
    }
    if ctx.method == Method::PATCH
        && [
            "update_name",
            "update_email",
            "update_password",
            "update_time_zone",
            "update_locale",
        ]
        .iter()
        .any(|s| ctx.params.route_path == format!("/settings/{s}"))
    {
        account::write(app, ctx, headers).await
    } else {
        Ok(layout::not_ported_response(
            &ctx.method,
            &ctx.params.fullpath,
            ctx.turbo_frame.as_deref(),
        ))
    }
}
