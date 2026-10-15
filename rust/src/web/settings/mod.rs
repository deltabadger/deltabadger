//! Settings on the existing authenticated browser pipeline.
pub mod account;
pub mod confirmation;
pub mod api;
pub mod clients;
pub mod keys;
pub mod validator;
pub mod mail;
pub mod preferences;
mod qr;
pub mod tax;
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
    if removed(&ctx.params.route_path) {
        return Ok(layout::missing());
    }
    if ctx.user().is_none() {
        return Ok(auth::unauthenticated(&ctx));
    }
    match ctx.params.route_path.as_str() {
        "/settings/edit_two_fa" => two_fa::handle(app, ctx, false).await,
        p if p.starts_with("/settings/api_key_permissions/") => keys::modal(app, ctx).await,
        p => match SECTIONS.iter().find(|section| p.strip_prefix("/settings/") == Some(**section)) {
            Some(section) => view::section(&app, &ctx, section, StatusCode::OK, None).await,
            None => Ok(layout::not_ported_response(&ctx.method, &ctx.params.fullpath, ctx.turbo_frame.as_deref())),
        },
    }
}

/// SettingsController::SECTIONS: the dialog's sections, each its own URL.
pub const SECTIONS: [&str; 5] = ["account", "api_keys", "connect", "mcp", "api"];

/// Routes Rails dropped with the settings dialog (it asks in place now): no route, so 404 as Rails answers one.
fn removed(path: &str) -> bool {
    ["/settings/confirm_destroy_api_key/", "/settings/confirm_revoke_mcp_client/"].iter().any(|prefix| path.starts_with(prefix))
        || ["/settings/confirm_destroy_coingecko_key", "/settings/destroy_coingecko_key", "/settings/update_coingecko_key"].contains(&path)
}
pub async fn write(
    State(app): State<App>,
    Extension(ctx): Extension<Ctx>,
    headers: HeaderMap,
) -> Result<Response, WebError> {
    if removed(&ctx.params.route_path) {
        return Ok(layout::missing());
    }
    if ctx.user().is_none() {
        return Ok(auth::unauthenticated(&ctx));
    }
    // R3: HTML settings use form parameters; JSON clients use /api routes.
    // Reject JSON uniformly before any settings writer can infer missing fields as defaults.
    use axum::response::IntoResponse;
    let json = match headers.get(axum::http::header::CONTENT_TYPE) {
        Some(value) => match value.to_str() {
            Ok(value) => value.split(';').next().is_some_and(|media|media.trim_ascii().eq_ignore_ascii_case("application/json")),
            Err(_) => return Ok(StatusCode::BAD_REQUEST.into_response()),
        },
        None => false,
    };
    if json { return Ok(StatusCode::BAD_REQUEST.into_response()); }
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
    if ctx.method == Method::PATCH && ctx.params.route_path == "/settings/update_wash_sale" {
        return tax::write(app, ctx, headers).await;
    }
    if ctx.method == Method::POST && ctx.params.route_path == "/settings/regenerate_api_token" {
        return api::regenerate(app, ctx).await;
    }
    if ctx.method == Method::PATCH
        && preferences::ACTIONS
            .iter()
            .any(|s| ctx.params.route_path == format!("/settings/{s}"))
    {
        return preferences::write(app, ctx, headers).await;
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
