//! `home#index` and `bots#index`: the dashboard of the user's bots.
use super::auth::{self, app_config};
use super::bot::{self, status, Bot, Kind};
use super::layout::{self, Ctx, Page};
use super::shell::{self, Shell};
use super::{colors, i18n, App, WebError};
use crate::enums::BotStatus;
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::Connection;
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

/// One option of shared/_segmented.html.erb.
pub struct SegmentedOption {
    pub value: &'static str,
    pub label: String,
    pub active: bool,
    pub href: Option<String>,
}

#[derive(Template)]
#[template(path = "shared/_segmented.html")]
pub struct Segmented<'a> {
    pub fluid: bool,
    pub label: String,
    /// What the reader's choice is remembered under in the browser, for a control that has a memory.
    pub key: Option<&'a str>,
    pub options: Vec<SegmentedOption>,
    pub links: bool,
}

enum Title {
    None,
    Text(String),
    /// Three or more symbols: three while they fit on one line, else two.
    Fit(String, String),
}

struct Chip {
    symbol: String,
    class: &'static str,
    color: String,
    asset_id: i64,
    /// The ids of the venues that list this member at the bot's quote, joined by commas.
    exchanges: String,
}

struct Tile {
    id: String,
    bot_id: i64,
    mergeable: bool,
    splittable: bool,
    exchange_id: i64,
    quote_id: String,
    quote_symbol: String,
    open_orders: bool,
    chips: Vec<Chip>,
    href: String,
    title: Title,
    exchange_svg: &'static str,
    pnl_id: String,
    pnl: Option<String>,
    label: String,
    bar: String,
    button: String,
}

#[derive(Template)]
#[template(path = "bots/index.html")]
struct IndexView<'a> {
    v: &'a Ctx,
    preferences: &'a str,
    bot_updates: &'a str,
    /// Some bot has traded, so the account's total is on its way (the broadcast endpoints are not served yet).
    pnl_pass: bool,
    headline: Option<String>,
    filters: Option<String>,
    select_mode: bool,
    can_merge: bool,
    can_split: bool,
    connected: String,
    names: String,
    no_partner: String,
    no_shared_exchange: String,
    other_exchange: String,
    tiles: Vec<Tile>,
    /// `{"bot_ids":[…]}`: the tiles whose figures the page asks for once it is connected.
    refresh: Option<String>,
}

/// `to_json` as ActiveSupport writes it into a page: `<`, `>` and `&` as \u escapes.
pub fn json(value: &Value) -> String {
    value.to_string().replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026")
}

/// MarketDataSettings.current_provider == 'deltabadger', and MarketDataSettings.configured?
pub fn market_data(c: &Connection, app: &App) -> Result<(bool, bool), WebError> {
    let setting = |key: &str| app_config(c, &app.cipher, key).map(|value| value.filter(|v| !v.trim().is_empty()));
    let provider = setting("market_data_provider")?;
    let deltabadger = app.config.market_data_url || provider.as_deref() == Some("deltabadger");
    let configured = if app.config.market_data_url { true } else if deltabadger { setting("market_data_url")?.is_some() && setting("market_data_token")?.is_some() } else { provider.is_some() };
    Ok((deltabadger, configured))
}

/// `Bot::Merge.mergeable?` and `Bot::Split.splittable?` for a bot `bot::refusal` has let through:
/// no rebalance, liquidation or redeploy is in flight for such a bot.
fn mergeable(bot: &Bot) -> (bool, bool) {
    let members = bot.memberships.iter().filter(|m| m.in_index).count();
    let eligible = !matches!(bot.status, BotStatus::Archived | BotStatus::Deleted | BotStatus::Executing);
    (eligible && members >= 1, eligible && members >= 2)
}

fn tile(c: &Connection, ctx: &Ctx, csrf: &str, bot: &Bot, market_data_configured: bool) -> Result<Tile, WebError> {
    let (mergeable, splittable) = mergeable(bot);
    let quote_id = bot.setting("quote_asset_id").and_then(Value::as_i64);
    let mut chips = vec![];
    if mergeable || splittable {
        // Bot::Merge.members: the current members, heaviest first.
        let mut members: Vec<&bot::Membership> = bot.memberships.iter().filter(|m| m.in_index).collect();
        members.sort_by(|a, b| b.target_allocation.cmp(&a.target_allocation));
        // Bot::Merge.venues_for: where each member trades at this quote, on a venue a user can still pick.
        let mut venues = c.prepare("SELECT DISTINCT t.exchange_id FROM tickers t JOIN exchanges e ON e.id = t.exchange_id \
                                    WHERE t.available = 1 AND t.trading_enabled = 1 AND e.available = 1 AND e.type IS NOT 'Exchanges::Bitmart' \
                                    AND t.base_asset_id = ?1 AND t.quote_asset_id IS ?2 ORDER BY t.exchange_id")?;
        for member in members {
            let ids = venues.query_map((member.asset.id, quote_id), |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
            chips.push(Chip {
                symbol: member.asset.symbol().to_string(),
                class: colors::ticker_class(member.asset.category.as_deref(), member.asset.color.as_deref()),
                color: colors::pill_color(member.asset.color.as_deref()).ok_or_else(|| WebError::Config(format!("asset {}: its colour is not a colour", member.asset.id)))?,
                asset_id: member.asset.id,
                exchanges: ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
            });
        }
    }
    let title = match bot.kind {
        Kind::Index if quote_id.is_some() && bot.setting("num_coins").is_some() => {
            Title::Text(bot.display_index_name().unwrap_or_else(|| i18n::text(ctx.locale, "bot.dca_index.setup.pick_index.top_coins", &[])))
        }
        Kind::Basket if quote_id.is_some() => {
            let symbols: Vec<&str> = bot.base_assets.iter().map(|asset| asset.symbol()).collect();
            if symbols.len() >= 3 {
                let more = if symbols.len() > 3 { format!(" +{}", symbols.len() - 3) } else { String::new() };
                Title::Fit(format!("{}{more}", symbols.iter().take(3).copied().collect::<Vec<_>>().join(", ")),
                           format!("{} +{}", symbols.iter().take(2).copied().collect::<Vec<_>>().join(", "), symbols.len() - 2))
            } else {
                Title::Text(symbols.join(", "))
            }
        }
        _ => Title::None,
    };
    let status = status::render(c, ctx, csrf, bot, market_data_configured)?;
    Ok(Tile {
        id: bot.dom_id("tile"), bot_id: bot.id, mergeable, splittable, exchange_id: bot.exchange.id,
        quote_id: quote_id.map(|id| id.to_string()).unwrap_or_default(), quote_symbol: bot.quote_symbol().unwrap_or("").to_string(),
        open_orders: mergeable && bot.has_waiting_orders, chips, href: ctx.path(&format!("/bots/{}", bot.id)), title,
        exchange_svg: bot::exchange_svg(&bot.exchange.name_id()), pnl_id: bot.dom_id("pnl"), pnl: None, label: bot.label.clone(),
        bar: status.bar, button: status.button,
    })
}

enum Listing {
    /// No bot that is not deleted: the welcome page.
    Empty { ask_admin_for_stocks: bool },
    Body(String),
    NotPorted(&'static str),
}

/// GET /bots.
pub async fn index(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    // With exactly one bot the list is that bot's page. Decided first: a redirect renders no form, so it gives the session no token.
    let user_id = user.id;
    if let [only] = app.db(move |c| Bot::ids(c, user_id)).await?.as_slice() {
        return Ok(layout::redirect(StatusCode::FOUND, &ctx.path(&format!("/bots/{only}"))));
    }
    let snapshot = crate::web::figure::loading::prepare(&app,user.id).await?;
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let listed = app.db(move |c| {
        let (ctx, user) = (&view, &owner);
        let shell = Shell::load(c, &inner, user)?;
        let ids = Bot::ids(c, user.id)?;
        if ids.is_empty() {
            // StockTradingSettings.active?: a hosted install, or a synced stock catalog (Exchange::STOCK_TYPES).
            let stocks_active = inner.config.market_data_url
                || app_config(c, &inner.cipher, "market_data_provider")?.as_deref() == Some("deltabadger")
                || c.query_row("SELECT EXISTS(SELECT 1 FROM tickers JOIN exchanges ON exchanges.id = tickers.exchange_id \
                                WHERE tickers.available = 1 AND exchanges.type IN ('Exchanges::Alpaca', 'Exchanges::Ibkr'))", [], |r| r.get(0))?;
            return Ok((shell, Listing::Empty { ask_admin_for_stocks: !stocks_active && !user.admin }));
        }
        let (deltabadger, configured) = market_data(c, &inner)?;
        let wash_sale: Option<bool> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [user.id], |r| r.get(0))?;
        let mut bots = vec![];
        for id in &ids {
            if let Some(reason) = bot::refusal(c, *id, wash_sale, deltabadger, if matches!(inner.figure_source,crate::web::figure::loading::Source::Disabled) { bot::For::Page } else { bot::For::FiguresPage })? { return Ok((shell, Listing::NotPorted(reason))); }
            if let Some(bot) = Bot::find(c, user.id, *id, bot::For::Page)? {
                if let Some(reason) = bot.unrendered() { return Ok((shell, Listing::NotPorted(reason))); }
                bots.push(bot);
            }
        }
        // From here on a page is rendered, and its forms carry the session's token.
        let csrf = ctx.csrf_token();
        let csrf = csrf.as_str();
        let working = |bot: &Bot| bot.working();
        let idle = |bot: &Bot| matches!(bot.status, BotStatus::Created | BotStatus::Stopped);
        let archived = |bot: &Bot| bot.status == BotStatus::Archived;
        let (total, has_active, has_inactive, has_archived) = (bots.iter().filter(|b| !archived(b)).count(), bots.iter().any(working), bots.iter().any(idle), bots.iter().any(archived));
        let filter = ctx.params.query("filter").unwrap_or("all");
        let listed: Vec<&Bot> = bots.iter().filter(|bot| match filter { "active" => working(bot), "inactive" => idle(bot), "archived" => archived(bot), _ => !archived(bot) }).collect();
        let filters = if (total > 1 && has_active && has_inactive) || has_archived {
            let names: &[&'static str] = if has_archived { &["all", "active", "inactive", "archived"] } else { &["all", "active", "inactive"] };
            let options = names.iter().map(|name| SegmentedOption {
                value: name, label: i18n::text(ctx.locale, &format!("bot_filters.{name}"), &[]), active: filter == *name,
                href: Some(format!("{}?filter={name}", ctx.path("/bots"))),
            }).collect();
            Some(Segmented { fluid: true, label: i18n::text(ctx.locale, "bot_filters.all", &[]), key: None, options, links: true }.render()?)
        } else {
            None
        };
        let (can_merge, can_split) = (listed.iter().filter(|bot| mergeable(bot).0).count() >= 2, listed.iter().any(|bot| mergeable(bot).1));
        // Bot::Merge.connected_exchange_ids, and `Exchange.tradeable.pluck(:id, :name).to_h`.
        let mut keys = c.prepare("SELECT exchange_id FROM api_keys WHERE user_id = ?1 AND key_type = 0 ORDER BY id")?;
        let mut connected: Vec<i64> = vec![];
        for exchange_id in keys.query_map([user.id], |r| r.get::<_, i64>(0))? {
            let exchange_id = exchange_id?;
            if !connected.contains(&exchange_id) { connected.push(exchange_id); }
        }
        let mut tradeable = c.prepare("SELECT id, name FROM exchanges WHERE available = 1 AND type IS NOT 'Exchanges::Bitmart' ORDER BY id")?;
        let mut names = serde_json::Map::new();
        for row in tradeable.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? {
            let (id, name) = row?;
            names.insert(id.to_string(), name.map_or(Value::Null, Value::String));
        }
        let literal = |key: &str, name: &str| i18n::text(ctx.locale, key, &[(name, i18n::Arg::Text(&format!("%{{{name}}}")))]);
        let figures = crate::web::figure::loading::render(c,user.id,&snapshot,ctx.locale,csrf,&ctx.path(""));
        let headline=figures.as_ref().and_then(|v|v["account"].as_str()).map(str::to_string);
        let mut tiles = vec![];
        for bot in &listed {
            let mut tile=tile(c,ctx,csrf,bot,configured)?;
            tile.pnl=figures.as_ref().and_then(|v|v["bots"][bot.id.to_string()]["tile"].as_str()).map(str::to_string);
            tiles.push(tile);
        }
        let (preferences, bot_updates) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id));
        let body = IndexView {
            headline,
            v: ctx, preferences: &preferences, bot_updates: &bot_updates,
            // User#global_pnl_snapshot with nothing computed: waiting as soon as any bot has an accepted order.
            pnl_pass: bots.iter().any(|bot| bot.has_submitted_orders),
            filters, select_mode: can_merge || can_split, can_merge, can_split, connected: json(&serde_json::json!(connected)), names: json(&Value::Object(names)),
            no_partner: literal("bot.merge.no_partner", "quote"), no_shared_exchange: literal("bot.merge.no_shared_exchange", "quote"),
            other_exchange: literal("bot.merge.other_exchange", "exchange"),
            refresh: (!listed.is_empty()).then(|| json(&serde_json::json!({ "bot_ids": listed.iter().map(|bot| bot.id).collect::<Vec<_>>() }))),
            tiles,
        }.render()?;
        Ok((shell, Listing::Body(body)))
    }).await;
    // One bot with a stored number this build does not read refuses the list, as any refused bot does.
    let (shell, listing) = match listed { Ok(listed) => listed, Err(error) => return layout::or_refused(&ctx, error) };
    let body = match listing {
        Listing::NotPorted(reason) => return Ok(layout::refused(&ctx, reason)),
        Listing::Body(body) => body,
        Listing::Empty { ask_admin_for_stocks } => {
            // Rails opens the new-bot wizard on the first bots page after a sign-in: `hide-chrome` on the body
            // (which hides the navbar and the main content until the modal closes) and a `src` on the modal
            // frame. There is no wizard in this build, and hidden chrome with nothing to close would leave an
            // empty page. So the flag is consumed, as in Rails, and the page is the one of any later visit.
            // Porting the wizard restores both and removes `without_wizard` from tests/pages.rs.
            ctx.session.lock().auto_open_bot_wizard = false;
            let (preferences, bot_updates) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id));
            EmptyView { v: &ctx, preferences: &preferences, bot_updates: &bot_updates, ask_admin_for_stocks }.render()?
        }
    };
    shell::application(&ctx, &ctx.csrf_token(), &user, &shell, Page { status: StatusCode::OK, body, flash_now: Vec::new() })
}
