//! The platform connection on Connect (Settings::PlatformConnectionsController), admin only, and the retired stocks writes
//! (SettingsController#update_stocks, #disconnect_stocks).
//!
//! Divergences:
//! - D-STOCKS-RETIRED: both stocks writes answer `head :unprocessable_entity` and write nothing, Rails' own answer when the
//!   catalogue comes from elsewhere. Here it always does (D-ALPACA-CATALOGUE), and an Alpaca key is a venue key on API keys
//!   (D-BROKERS-AS-VENUES): no page of this build posts to either route.
//! - D-PLATFORM-HOSTED-KEPT: a disconnect on a hosted install (MARKET_DATA_URL set) is refused and writes nothing. Rails
//!   deletes the claim's rows there, the exchange proxies among them, though its widget offers no disconnect.
use super::{keys, market_data};
use crate::web::{auth, layout::Ctx, setup, turbo, App, WebError};
use axum::{
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, Transaction, TransactionBehavior};

pub const PATH: &str = "/settings/platform_connection";

/// `head :unprocessable_entity`, in the format the request negotiated.
fn unprocessable(headers: &axum::http::HeaderMap) -> Response {
    // `head` names the format without a charset.
    let format = if turbo::accepts_stream(headers) { "text/vnd.turbo-stream.html" } else { "text/html" };
    (StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, format)]).into_response()
}

pub async fn write(app: App, ctx: Ctx, headers: &axum::http::HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user() else { return Ok(auth::unauthenticated(&ctx)) };
    // AdminOnly#require_admin!.
    if !user.admin { return Ok(market_data::head(StatusCode::FORBIDDEN)); }
    match (&ctx.method, ctx.params.route_path.as_str()) {
        (&Method::POST, PATH) => create(app, ctx).await,
        (&Method::DELETE, PATH) if app.config.market_data_url => Ok(unprocessable(headers)),
        (&Method::DELETE, PATH) => destroy(app, ctx).await,
        // D-STOCKS-RETIRED.
        _ => Ok(unprocessable(headers)),
    }
}

/// Platform::RedeemClaim (setup's, without its open-setup check), then the widget: the notice on success, the form with
/// the claim's error at 422 otherwise.
async fn create(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let outcome = setup::redeem(&app, ctx.params.form("claim_code").map_or(serde_json::Value::Null, |code| code.into()), ctx.now, false).await?;
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let error = outcome.err();
    let (widget, proxies) = app
        .db(move |c| Ok((keys::market_data(c, &inner, &v, &csrf, false, error.as_deref())?, error.is_none().then(|| proxies(c)).transpose()?)))
        .await?;
    match proxies {
        Some(proxies) => market_data::widget_and_flash(&ctx, &widget, Some(if proxies { "settings.platform.connected_with_proxies" } else { "settings.platform.connected_without_proxies" })),
        // Rails prepends the flash only `if flash.any?`: a message the session still carries, none of its own.
        None => {
            let pending = crate::web::flash::take(&ctx.session, &[]);
            let mut body = turbo::stream("replace", "market_data_settings", widget.trim_end());
            if !pending.is_empty() { body += &turbo::prepend_flash(crate::web::flash::render(&pending)?.trim_end()); }
            Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response())
        }
    }
}

/// AppConfig.platform_proxies_configured?: the exact, case-sensitive `proxy_` prefix (`start_with?`).
pub(super) fn proxies(c: &Connection) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE substr(key, 1, 6) = 'proxy_')", [], |r| r.get(0))?)
}

/// The claim's rows: the provider when it is the platform's, the data-api URL and token, every proxy and the connection time.
pub fn forget(tx: &Connection, cipher: &crate::crypto::Cipher) -> Result<(), String> {
    let env = crate::market_data::Env::default();
    if crate::market_data::read(tx, cipher, &env)?.deltabadger() { crate::app_config::delete(tx, "market_data_provider")?; }
    for key in ["market_data_url", "market_data_token", "platform_connected_at"] { crate::app_config::delete(tx, key)?; }
    tx.execute("DELETE FROM app_configs WHERE substr(key, 1, 6) = 'proxy_'", []).map_err(|e| e.to_string())?;
    Ok(())
}

async fn destroy(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let widget = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            forget(&tx, &inner.cipher).map_err(WebError::Config)?;
            let widget = keys::market_data(&tx, &inner, &v, &csrf, false, None)?;
            tx.commit()?;
            Ok(widget)
        })
        .await?;
    market_data::widget_and_flash(&ctx, &widget, Some("settings.platform.disconnected"))
}
