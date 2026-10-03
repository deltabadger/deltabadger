//! `home#index` and the part of `bots#index` served so far: the page of an account with no bots.
use super::auth::{self, app_config};
use super::layout::{self, Ctx, Page};
use super::shell::{self, Shell};
use super::{App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;

/// Tracker::UnfundedCash.cash?: its FIAT and STABLECOINS (pinned by tests/web.rs against Rails' lists).
pub const CASH: [&str; 21] = [
    "AED", "AUD", "BGN", "BUSD", "CAD", "CHF", "CZK", "DAI", "DKK", "EUR", "FDUSD", "GBP", "JPY", "PLN", "PYUSD", "RLUSD", "SEK", "TUSD", "USD",
    "USDC", "USDT",
];

/// User#show_cash?: `tracker_settings&.dig('show_cash').present?` on the column's JSON text.
pub fn show_cash(tracker_settings: Option<&str>) -> bool {
    let settings: Value = tracker_settings.and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null);
    match settings.get("show_cash") {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(entries)) => !entries.is_empty(),
        Some(Value::Bool(true) | Value::Number(_)) => true,
    }
}


/// GET /: to the bots page when signed in, else to the login page. Both keep the request's locale.
pub async fn home(Extension(ctx): Extension<Ctx>) -> Response {
    layout::redirect(StatusCode::FOUND, &ctx.path(if ctx.user().is_some() { "/bots" } else { "/login" }))
}

#[derive(Template)]
#[template(path = "bots/empty.html")]
struct EmptyView<'a> {
    v: &'a Ctx,
    preferences: &'a str,
    bot_updates: &'a str,
    /// `!StockTradingSettings.active? && !current_user.admin?`
    ask_admin_for_stocks: bool,
}

struct Facts {
    shell: Shell,
    /// Bots that are not deleted (status 3), archived ones included.
    bots: i64,
    stocks_active: bool,
}

/// GET /bots.
pub async fn index(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, user_id) = (app.clone(), user.id);
    let owner = user.clone();
    let facts = app.db(move |c| {
        Ok(Facts {
            shell: Shell::load(c, &inner, &owner)?,
            bots: c.query_row("SELECT count(*) FROM bots WHERE user_id = ?1 AND status != 3", [user_id], |r| r.get(0))?,
            // StockTradingSettings.active?: a hosted install, or a synced stock catalog (Exchange::STOCK_TYPES).
            stocks_active: inner.config.market_data_url
                || app_config(c, &inner.cipher, "market_data_provider")?.as_deref() == Some("deltabadger")
                || c.query_row("SELECT EXISTS(SELECT 1 FROM tickers JOIN exchanges ON exchanges.id = tickers.exchange_id \
                                WHERE tickers.available = 1 AND exchanges.type IN ('Exchanges::Alpaca', 'Exchanges::Ibkr'))", [], |r| r.get(0))?,
        })
    }).await?;
    // The bot list is a later task. Refuse, never approximate.
    if facts.bots != 0 {
        return Ok(layout::not_ported_response(&ctx.method, &ctx.params.fullpath, ctx.turbo_frame.as_deref()));
    }
    // Rails opens the new-bot wizard on the first bots page after a sign-in: `hide-chrome` on the body
    // (which hides the navbar and the main content until the modal closes) and a `src` on the modal
    // frame. There is no wizard in this build, and hidden chrome with nothing to close would leave an
    // empty page. So the flag is consumed, as in Rails, and the page is the one of any later visit.
    // Porting the wizard restores both and removes `without_wizard` from tests/pages.rs.
    ctx.session.lock().auto_open_bot_wizard = false;
    let csrf = ctx.csrf_token();
    let (preferences, bot_updates) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id));
    let body = EmptyView { v: &ctx, preferences: &preferences, bot_updates: &bot_updates, ask_admin_for_stocks: !facts.stocks_active && !user.admin }.render()?;
    let page = Page { status: StatusCode::OK, body, flash_now: Vec::new() };
    shell::application(&ctx, &csrf, &user, &facts.shell, page)
}
