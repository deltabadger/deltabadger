//! `home#index` and the part of `bots#index` this plan serves: the page of an account with no bots.
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

/// Whether TrackerHelper#allocation_icon_arcs has anything to draw: a priced holding that is not
/// cash, or any priced holding when the user's tracker shows cash. Otherwise the icon is a plain circle.
pub fn tracker_ring(priced_symbols: &[Option<String>], show_cash: bool) -> bool {
    priced_symbols.iter().any(|symbol| show_cash || !symbol.as_deref().is_some_and(|symbol| CASH.contains(&symbol)))
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
    /// Bots that are not deleted (status 3), archived ones included.
    bots: i64,
    /// Whether the navbar's tracker icon would be a ring of holdings, which this plan does not draw.
    tracker_ring: bool,
    syncing: bool,
    stocks_active: bool,
}

/// GET /bots.
pub async fn index(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, user_id) = (app.clone(), user.id);
    let facts = app.db(move |c| {
        // AccountBalance.for_user(user).priced.joins(:asset), by symbol.
        let mut priced = c.prepare("SELECT DISTINCT assets.symbol FROM account_balances JOIN assets ON assets.id = account_balances.asset_id \
                                    WHERE account_balances.user_id = ?1 AND account_balances.usd_value IS NOT NULL AND account_balances.usd_value > 0")?;
        let priced_symbols = priced.query_map([user_id], |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?;
        let tracker_settings: Option<String> = c.query_row("SELECT tracker_settings FROM users WHERE id = ?1", [user_id], |r| r.get(0))?;
        Ok(Facts {
            bots: c.query_row("SELECT count(*) FROM bots WHERE user_id = ?1 AND status != 3", [user_id], |r| r.get(0))?,
            tracker_ring: tracker_ring(&priced_symbols, show_cash(tracker_settings.as_deref())),
            syncing: app_config(c, &inner.cipher, "setup_sync_status")?.as_deref() == Some("in_progress"),
            // StockTradingSettings.active?: a hosted install, or a synced stock catalog (Exchange::STOCK_TYPES).
            stocks_active: inner.config.market_data_url
                || app_config(c, &inner.cipher, "market_data_provider")?.as_deref() == Some("deltabadger")
                || c.query_row("SELECT EXISTS(SELECT 1 FROM tickers JOIN exchanges ON exchanges.id = tickers.exchange_id \
                                WHERE tickers.available = 1 AND exchanges.type IN ('Exchanges::Alpaca', 'Exchanges::Ibkr'))", [], |r| r.get(0))?,
        })
    }).await?;
    // The bot list and the tracker ring are later plans. Refuse, never approximate.
    if facts.bots != 0 || facts.tracker_ring {
        return Ok(layout::not_ported_response(&ctx.method, &ctx.params.fullpath, ctx.turbo_frame.as_deref()));
    }
    // Rails opens the new-bot wizard on the first bots page after a sign-in: `hide-chrome` on the body
    // (which hides the navbar and the main content until the modal closes) and a `src` on the modal
    // frame. There is no wizard in this build, and hidden chrome with nothing to close would leave an
    // empty page. So the flag is consumed, as in Rails, and the page is the one of any later visit.
    // The plan that ports the wizard restores both and removes `without_wizard` from tests/pages.rs.
    ctx.session.lock().auto_open_bot_wizard = false;
    let csrf = ctx.csrf_token();
    let (preferences, bot_updates) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id));
    let body = EmptyView { v: &ctx, preferences: &preferences, bot_updates: &bot_updates, ask_admin_for_stocks: !facts.stocks_active && !user.admin }.render()?;
    let page = Page { status: StatusCode::OK, body, flash_now: Vec::new() };
    shell::application(&ctx, &csrf, &user, &Shell { syncing: facts.syncing, bot_count: 0 }, page)
}
