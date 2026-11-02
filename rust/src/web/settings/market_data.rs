//! The container's market data: SettingsController#update_market_data, #disconnect_market_data and #resync_assets
//! (settings_controller.rb:186-235), admin only, and the CoinGecko key the tracker's and the index wizard's CoinGecko steps
//! save (tracker_controller.rb:160-185, bots/dca_indexes/setup_coingeckos_controller.rb).
use super::keys;
use crate::{
    jobs::{coingecko, setup_sync},
    venue::http::{HttpRequest, HttpResponse, Transport, TransportError},
    web::{auth, flash, layout::Ctx, turbo, App, WebError},
};
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, Transaction, TransactionBehavior};

/// `App::with_venue_key_url(COINGECKO, server)`: tests send CoinGecko's hosts to `<server>/<host>`.
pub const COINGECKO: &str = "CoinGecko";

/// The live transport with each host under a test server's path.
struct Rebased { live: coingecko::LiveTransport, at: String }

impl Transport for Rebased {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.send_limited(r, usize::MAX).await }
    async fn send_limited(&self, r: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> {
        let mut r = r.clone();
        r.base = format!("{}/{}", self.at, r.base.trim_start_matches("https://"));
        self.live.send_limited(&r, limit).await
    }
}

/// `validate_coingecko_api_key` as coingecko::check judges it (D-CG-KEY-CHECK-UNCACHED, D-CG-KEY-CHECK-NO-500).
pub(crate) async fn key_valid(app: &App, key: &str) -> bool {
    match app.venue_key_url_set(COINGECKO) {
        Some(at) => coingecko::check(&coingecko::CoinGecko::new(key, Rebased { live: coingecko::LiveTransport::new(key), at })).await,
        None => coingecko::check(&coingecko::CoinGecko::live(key)).await,
    }
}

/// `flash.now[:alert] = t('setup.invalid_coingecko_api_key')`, prepended with 422.
pub(crate) fn invalid_key(ctx: &Ctx) -> Result<Response, WebError> {
    let markup = flash::render(&flash::take(&ctx.session, &[(flash::ALERT, ctx.t("setup.invalid_coingecko_api_key"))]))?;
    let mut response = stream(turbo::prepend_flash(markup.trim_end()));
    *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    Ok(response)
}

fn stream(body: String) -> Response { ([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response() }

/// `head status`.
pub(crate) fn head(status: StatusCode) -> Response { (status, [(header::CONTENT_TYPE, "text/html")]).into_response() }

fn config(result: Result<(), String>) -> Result<(), WebError> { result.map_err(WebError::Config) }

/// D-CG-SETUP-DURABLE: the setup request in the caller's transaction; `wake` after its commit.
fn request_setup(tx: &Connection, app: &App) -> Result<(), WebError> { config(setup_sync::request(tx, app.now())) }

fn wake(app: &App) {
    // Setup::SeedAndSyncJob.perform_later, after the commit.
    if let Ok(jobs) = app.job_wakers() { jobs.wake(setup_sync::NAME, None, None); }
}

/// The tracker's and the wizard's save of a checked key: `AppConfig.coingecko_api_key = key`, the provider, and the setup
/// run (Setup::SeedAndSyncJob), in one transaction.
pub(crate) async fn save_key(app: &App, key: String) -> Result<(), WebError> {
    let inner = app.clone();
    app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        config(crate::app_config::set(&tx, &inner.cipher, "coingecko_api_key", &key, inner.now()))?;
        config(crate::app_config::set(&tx, &inner.cipher, "market_data_provider", "coingecko", inner.now()))?;
        request_setup(&tx, &inner)?;
        tx.commit()?;
        wake(&inner);
        Ok(())
    })
    .await
}

/// AppConfig.coingecko_api_key: the row's value when a row exists (NULL reads as nil), else COINGECKO_API_KEY or ''.
pub(super) fn stored_key(c: &Connection, app: &App) -> Result<String, WebError> {
    if crate::app_config::exists(c, "coingecko_api_key").map_err(WebError::Config)? {
        return Ok(auth::app_config(c, &app.cipher, "coingecko_api_key")?.unwrap_or_default());
    }
    Ok(app.config.market_data_env.coingecko_key.clone().unwrap_or_default())
}

/// MarketDataSettings.deltabadger_credentials_available?: the environment's feed, or a claimed platform with its URL and token.
fn credentials_available(c: &Connection, app: &App) -> Result<bool, WebError> {
    let env = &app.config.market_data_env;
    let present = |v: &Option<String>| v.as_deref().is_some_and(|s| !crate::ruby::blank(s));
    if present(&env.url) { return Ok(true); }
    // `record ? record.value : ENV.fetch(var, '')`.
    let setting = |key: &str, fallback: &Option<String>| -> Result<Option<String>, WebError> {
        if crate::app_config::exists(c, key).map_err(WebError::Config)? { auth::app_config(c, &app.cipher, key) } else { Ok(fallback.clone()) }
    };
    Ok(present(&auth::app_config(c, &app.cipher, "platform_connected_at")?)
        && present(&setting("market_data_url", &env.url)?)
        && present(&setting("market_data_token", &env.token)?))
}

pub async fn write(app: App, ctx: Ctx, headers: &axum::http::HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user() else { return Ok(auth::unauthenticated(&ctx)) };
    // AdminOnly#require_admin!.
    if !user.admin { return Ok(head(StatusCode::FORBIDDEN)); }
    match ctx.params.route_path.as_str() {
        "/settings/update_market_data" => update(app, ctx, turbo::accepts_stream(headers)).await,
        "/settings/disconnect_market_data" => disconnect(app, ctx).await,
        _ => resync(app, ctx).await,
    }
}

/// A blank provider switches market data off; CoinGecko takes the posted key, else the stored one, else opens its key form;
/// the platform's feed needs its credentials (`head :unprocessable_entity` otherwise). Anything else changes nothing.
async fn update(app: App, ctx: Ctx, stream_wanted: bool) -> Result<Response, WebError> {
    let provider = ctx.params.form("market_data_provider").map(str::to_string);
    let posted = ctx.params.form("coingecko_api_key").filter(|key| !crate::ruby::blank(key)).map(str::to_string);
    // D-CG-SETTINGS-KEY-CHECKED: a key posted here is checked as the tracker's and the wizard's are; nothing is written for a
    // key CoinGecko refuses. The stored key a switch back uses is not checked again (Rails).
    if provider.as_deref() == Some("coingecko") {
        if let Some(key) = &posted {
            if !key_valid(&app, key).await { return invalid_key(&ctx); }
        }
    }
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let done = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            let (cipher, now) = (&inner.cipher, inner.now());
            let mut show_form = false;
            let mut requested = false;
            let notice = match provider.as_deref() {
                p if p.is_none_or(crate::ruby::blank) => {
                    config(crate::app_config::delete(&tx, "market_data_provider"))?;
                    Some("settings.market_data.disabled")
                }
                Some("coingecko") => {
                    let key = match posted { Some(key) => key, None => stored_key(&tx, &inner)? };
                    if crate::ruby::blank(&key) {
                        config(crate::app_config::delete(&tx, "market_data_provider"))?;
                        show_form = true;
                        None
                    } else {
                        config(crate::app_config::set(&tx, cipher, "market_data_provider", "coingecko", now))?;
                        config(crate::app_config::set(&tx, cipher, "coingecko_api_key", &key, now))?;
                        // D-CG-SETTINGS-SAVE-SYNCS: Rails' update_market_data enqueues nothing, so a fresh key syncs
                        // nothing until the schedules fire; here it asks for the setup run the tracker's save asks for.
                        request_setup(&tx, &inner)?;
                        requested = true;
                        Some("settings.market_data.updated")
                    }
                }
                Some("deltabadger") => {
                    if !credentials_available(&tx, &inner)? { return Ok(None); }
                    config(crate::app_config::set(&tx, cipher, "market_data_provider", "deltabadger", now))?;
                    Some("settings.market_data.updated")
                }
                _ => None,
            };
            let widget = keys::market_data(&tx, &inner, &v, &csrf, show_form)?;
            tx.commit()?;
            if requested { wake(&inner); }
            Ok(Some((widget, notice)))
        })
        .await?;
    let Some((widget, notice)) = done else {
        // `head :unprocessable_entity`, in the format the request negotiated.
        let format = if stream_wanted { "text/vnd.turbo-stream.html" } else { "text/html" };
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, format)]).into_response());
    };
    widget_and_flash(&ctx, &widget, notice)
}

/// D-CG-DISCONNECT-KEEPS-PLATFORM: the provider and the key, not a claimed platform's `market_data_url`/`_token`
/// (Rails' clear_market_data_settings! drops them too).
async fn disconnect(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let widget = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            config(crate::app_config::delete(&tx, "market_data_provider"))?;
            config(crate::app_config::set(&tx, &inner.cipher, "coingecko_api_key", "", inner.now()))?;
            let widget = keys::market_data(&tx, &inner, &v, &csrf, false)?;
            tx.commit()?;
            Ok(widget)
        })
        .await?;
    widget_and_flash(&ctx, &widget, Some("settings.market_data.disconnected"))
}

/// `setup_sync_status = in_progress` and the setup run; the syncing notice (layouts/_flash_syncing) prepended.
async fn resync(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let inner = app.clone();
    app.db(move |c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        config(crate::app_config::set(&tx, &inner.cipher, "setup_sync_status", "in_progress", inner.now()))?;
        request_setup(&tx, &inner)?;
        tx.commit()?;
        wake(&inner);
        Ok(())
    })
    .await?;
    let syncing = format!(
        "{}\n<div id=\"flash-syncing\" class=\"salert salert--syncing\" role=\"alert\">\n  <div class=\"loader--syncing\"></div>\n  {}\n</div>",
        ctx.stream_from("settings_sync"),
        ctx.t("setup.syncing_message")
    );
    Ok(stream(turbo::prepend_flash(&syncing)))
}

/// The widget replaced in its frame, and the flash prepended.
fn widget_and_flash(ctx: &Ctx, widget: &str, notice: Option<&str>) -> Result<Response, WebError> {
    let now: Vec<(&str, String)> = notice.map(|key| (flash::NOTICE, ctx.t(key))).into_iter().collect();
    let markup = flash::render(&flash::take(&ctx.session, &now))?;
    Ok(stream(turbo::stream("replace", "market_data_settings", widget.trim_end()) + &turbo::prepend_flash(markup.trim_end())))
}
