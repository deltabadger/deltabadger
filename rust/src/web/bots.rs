//! `home#index` and `bots#index`: the dashboard of the user's bots.
use super::auth::{self, app_config};
use super::bot::{self, status, Bot, Kind};
use super::layout::{self, Ctx, Page};
use super::shell::{self, Shell};
use super::{colors, i18n, App, WebError};
use crate::enums::{BotStatus, BOT_WORKING};
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
}

/// One option of shared/_segmented.html.erb.
pub struct SegmentedOption {
    pub value: std::borrow::Cow<'static, str>,
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
    /// The `<turbo-frame>` a link group's options load into instead of a whole page (3.0, Rust only): the control is
    /// outside it and stays, so its chip slides instead of being rebuilt with the page.
    pub frame: Option<&'a str>,
}

/// The bot list's frame: the tiles the status filter scopes.
pub const SCOPE_FRAME: &str = "bots_scope";

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
    progress: String,
    button: String,
}

#[derive(Template)]
#[template(path = "bots/index.html")]
struct IndexView<'a> {
    v: &'a Ctx,
    preferences: &'a str,
    bot_updates: &'a str,
    /// No total yet and some bot has traded, so the account's total is on its way: the page asks for it.
    pnl_pass: bool,
    /// @global_pnl_loading: the headline itself still waits (the ask retries while its loading mark stands).
    pnl_loading: bool,
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
    /// A load of the scope frame alone: the head's actions again, as a stream (they stand outside the frame).
    stream: String,
}

/// The page head's actions: New, and Merge/Split when the listed bots allow them (`bots_actions`).
#[derive(Template)]
#[template(path = "bots/_actions.html")]
struct Actions<'a> { v: &'a Ctx, select_mode: bool, can_merge: bool, can_split: bool }

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

/// `Bot::Merge.mergeable?` and `Bot::Split.splittable?` for a bot `bot::refusal` has let through: neither while a
/// rebalance swap is pending (bot/merge.rb, bot/split.rb); a liquidation or redeploy in flight is refused before.
pub(crate) fn mergeable(bot: &Bot) -> (bool, bool) {
    let members = bot.memberships.iter().filter(|m| m.in_index).count();
    let eligible = !matches!(bot.status, BotStatus::Archived | BotStatus::Deleted | BotStatus::Executing) && !bot.rebalance_pending();
    (eligible && members >= 1, eligible && members >= 2)
}

/// Bot::Merge.venues_for, for one asset: where it trades at this quote, on a venue a user can still pick, in id order. By the
/// base's listings (`+` keeps the planner off the quote's index: every listing in USD, once per member).
pub(crate) fn venues_for(c: &Connection, asset: i64, quote: Option<i64>) -> Result<Vec<i64>, WebError> {
    let mut venues = c.prepare_cached("SELECT DISTINCT t.exchange_id FROM tickers t JOIN exchanges e ON e.id = t.exchange_id \
                                       WHERE t.available = 1 AND t.trading_enabled = 1 AND e.available = 1 AND e.type IS NOT 'Exchanges::Bitmart' \
                                       AND t.base_asset_id = ?1 AND +t.quote_asset_id IS ?2 ORDER BY t.exchange_id")?;
    let ids = venues.query_map((asset, quote), |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Bot::Merge.connected_exchange_ids: the exchanges the user has a trading key on, in the order the keys were added.
pub(crate) fn connected_exchange_ids(c: &Connection, user: i64) -> Result<Vec<i64>, WebError> {
    let mut keys = c.prepare("SELECT exchange_id FROM api_keys WHERE user_id = ?1 AND key_type = 0 ORDER BY id")?;
    let mut connected: Vec<i64> = vec![];
    for exchange_id in keys.query_map([user], |r| r.get::<_, i64>(0))? {
        let exchange_id = exchange_id?;
        if !connected.contains(&exchange_id) { connected.push(exchange_id); }
    }
    Ok(connected)
}

fn tile(c: &Connection, ctx: &Ctx, csrf: &str, bot: &Bot, market_data_configured: bool) -> Result<Tile, WebError> {
    let (mergeable, splittable) = mergeable(bot);
    let quote_id = bot.setting("quote_asset_id").and_then(Value::as_i64);
    let mut chips = vec![];
    if mergeable || splittable {
        // Bot::Merge.members: the current members, heaviest first.
        let mut members: Vec<&bot::Membership> = bot.memberships.iter().filter(|m| m.in_index).collect();
        members.sort_by(|a, b| b.target_allocation.cmp(&a.target_allocation));
        for member in members {
            let ids = venues_for(c, member.asset.id, quote_id)?;
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
        // `bot.signal? && bot.quote_asset_id.present? && bot.base_asset_id.present?`: the base's symbol.
        Kind::Signal if quote_id.is_some() && bot.setting("base_asset_id").is_some() => Title::Text(bot.base_asset().map(|asset| asset.symbol().to_string()).unwrap_or_default()),
        _ => Title::None,
    };
    let status = status::render(c, ctx, csrf, bot, market_data_configured)?;
    Ok(Tile {
        id: bot.dom_id("tile"), bot_id: bot.id, mergeable, splittable, exchange_id: bot.exchange.id,
        quote_id: quote_id.map(|id| id.to_string()).unwrap_or_default(), quote_symbol: bot.quote_symbol().unwrap_or("").to_string(),
        open_orders: mergeable && bot.has_waiting_orders, chips, href: ctx.path(&format!("/bots/{}", bot.id)), title,
        exchange_svg: bot::exchange_svg(&bot.exchange.name_id()), pnl_id: bot.dom_id("pnl"), pnl: None, label: bot.label.clone(),
        bar: status.bar, progress: status.progress, button: status.tile_button,
    })
}

/// A bot these pages refuse, as the dashboard lists it: from its row alone, linked to its own (refused) page,
/// with no figures, controls or merge (3.0, D-DASHBOARD-REFUSED-TILE).
struct Refused {
    id: i64,
    status: BotStatus,
    label: String,
    exchange_id: i64,
    exchange: String,
}

impl Refused {
    fn load(c: &Connection, id: i64, locale: &str) -> Result<Refused, WebError> {
        let (status, label, exchange_id, class): (i64, Option<String>, Option<i64>, Option<String>) = c.query_row(
            "SELECT b.status, b.label, b.exchange_id, e.type FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        let label = label.filter(|label| !label.trim().is_empty()).unwrap_or_else(|| i18n::text(locale, "bot.new", &[]));
        let exchange = bot::Exchange { id: exchange_id.unwrap_or_default(), name: String::new(), class: class.unwrap_or_default(), maker_fee: None }.name_id();
        Ok(Refused { id, status: BotStatus::from_i64(status).unwrap_or(BotStatus::Stopped), label, exchange_id: exchange_id.unwrap_or_default(), exchange })
    }

    fn tile(&self, ctx: &Ctx) -> Tile {
        Tile {
            id: format!("tile_bot_{}", self.id), bot_id: self.id, mergeable: false, splittable: false, exchange_id: self.exchange_id,
            quote_id: String::new(), quote_symbol: String::new(), open_orders: false, chips: vec![], href: ctx.path(&format!("/bots/{}", self.id)),
            title: Title::None, exchange_svg: bot::exchange_svg(&self.exchange), pnl_id: format!("pnl_bot_{}", self.id), pnl: Some(String::new()),
            label: self.label.clone(), bar: String::new(), progress: String::new(), button: String::new(),
        }
    }
}

enum Listing {
    /// No bot that is not deleted: the welcome page (D-STOCKS-NO-GATE: without Rails' "ask your admin" line).
    Empty,
    Body(String),
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
        if ids.is_empty() { return Ok((shell, Listing::Empty)); }
        let (deltabadger, configured) = market_data(c, &inner)?;
        let wash_sale: Option<bool> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [user.id], |r| r.get(0))?;
        // A bot these pages refuse is listed by its row alone, and only its own page is refused (3.0, D-DASHBOARD-REFUSED-TILE).
        let asked = if matches!(inner.figure_source,crate::web::figure::loading::Source::Disabled) { bot::For::Page } else { bot::For::FiguresPage };
        let mut entries: Vec<Result<Bot, Refused>> = vec![];
        for id in &ids {
            let found = match bot::refusal(c, *id, wash_sale, deltabadger, asked) {
                Ok(Some(reason)) => Err(reason),
                Ok(None) => match Bot::find(c, user.id, *id, bot::For::Page, ctx.locale) {
                    Ok(Some(bot)) => bot.unrendered().map_or(Ok(Some(bot)), Err),
                    Ok(None) => Ok(None),
                    Err(error) if bot::unreadable(&error) => Err(bot::UNREADABLE),
                    Err(error) => return Err(error),
                },
                Err(error) if bot::unreadable(&error) => Err(bot::UNREADABLE),
                Err(error) => return Err(error),
            };
            match found {
                Ok(Some(bot)) => entries.push(Ok(bot)),
                Ok(None) => {}
                Err(reason) => {
                    eprintln!("deltabadger: bot {id} is listed but not served: {reason}");
                    entries.push(Err(Refused::load(c, *id, ctx.locale)?));
                }
            }
        }
        let bots: Vec<&Bot> = entries.iter().filter_map(|entry| entry.as_ref().ok()).collect();
        // From here on a page is rendered, and its forms carry the session's token.
        let csrf = ctx.csrf_token();
        let csrf = csrf.as_str();
        let status = |entry: &Result<Bot, Refused>| match entry { Ok(bot) => bot.status, Err(refused) => refused.status };
        let working = |entry: &Result<Bot, Refused>| BOT_WORKING.contains(&status(entry));
        let idle = |entry: &Result<Bot, Refused>| matches!(status(entry), BotStatus::Created | BotStatus::Stopped);
        let archived = |entry: &Result<Bot, Refused>| status(entry) == BotStatus::Archived;
        let (total, has_active, has_inactive, has_archived) = (entries.iter().filter(|e| !archived(e)).count(), entries.iter().any(working), entries.iter().any(idle), entries.iter().any(archived));
        let filter = ctx.params.query("filter").unwrap_or("all");
        let shown: Vec<&Result<Bot, Refused>> = entries.iter().filter(|entry| match filter { "active" => working(entry), "inactive" => idle(entry), "archived" => archived(entry), _ => !archived(entry) }).collect();
        let filters = if (total > 1 && has_active && has_inactive) || has_archived {
            let names: &[&'static str] = if has_archived { &["all", "active", "inactive", "archived"] } else { &["all", "active", "inactive"] };
            let options = names.iter().map(|name| SegmentedOption {
                value: (*name).into(), label: i18n::text(ctx.locale, &format!("bot_filters.{name}"), &[]), active: filter == *name,
                href: Some(format!("{}?filter={name}", ctx.path("/bots"))),
            }).collect();
            Some(Segmented { fluid: true, label: i18n::text(ctx.locale, "bot_filters.label", &[]), key: None, options, links: true, frame: Some(SCOPE_FRAME) }.render()?)
        } else {
            None
        };
        // Bot::Merge.connected_exchange_ids, and `Exchange.tradeable.pluck(:id, :name).to_h`.
        let connected = connected_exchange_ids(c, user.id)?;
        let mut tradeable = c.prepare("SELECT id, name FROM exchanges WHERE available = 1 AND type IS NOT 'Exchanges::Bitmart' ORDER BY id")?;
        let mut names = serde_json::Map::new();
        for row in tradeable.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))? {
            let (id, name) = row?;
            names.insert(id.to_string(), name.map_or(Value::Null, Value::String));
        }
        let literal = |key: &str, name: &str| i18n::text(ctx.locale, key, &[(name, i18n::Arg::Text(&format!("%{{{name}}}")))]);
        let figures = crate::web::figure::loading::render_page(&inner,c,user.id,&snapshot,ctx.locale,csrf,&ctx.path(""))?;
        let history_loading=figures.as_ref().is_some_and(|v|v["history_loading"]==true);
        let pnl_loading=figures.as_ref().is_none_or(|v|!v["account"].is_string()) && bots.iter().any(|bot| bot.has_submitted_orders);
        let headline=figures.as_ref().and_then(|v|v["account_page"].as_str().or_else(||v["account"].as_str())).map(str::to_string);
        let mut tiles = vec![];
        let mut cold = vec![];
        for entry in &shown {
            match entry {
                Ok(bot) => match tile(c,ctx,csrf,bot,configured) {
                    Ok(mut tile) => {
                        tile.pnl=figures.as_ref().and_then(|v|v["bots"][bot.id.to_string()]["tile"].as_str()).map(str::to_string);
                        // BotsController#index: only the tiles still without figures are asked for (@tile_refresh_ids).
                        if tile.pnl.is_none() { cold.push(bot.id); }
                        tiles.push(tile);
                    }
                    // A number the tile reads (a cap's sum, say) this build does not: that bot's tile is plain, as a refused bot's.
                    Err(error) if bot::unreadable(&error) => {
                        eprintln!("deltabadger: bot {} is listed but not served: {}", bot.id, bot::UNREADABLE);
                        tiles.push(Refused::load(c, bot.id, ctx.locale)?.tile(ctx));
                    }
                    Err(error) => return Err(error),
                },
                Err(refused) => tiles.push(refused.tile(ctx)),
            }
        }
        // From the tiles as drawn: a plain tile (a refused bot) offers neither.
        let (can_merge, can_split) = (tiles.iter().filter(|tile| tile.mergeable).count() >= 2, tiles.iter().any(|tile| tile.splittable));
        let (preferences, bot_updates) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id));
        let select_mode = can_merge || can_split;
        let stream = if ctx.turbo_frame.as_deref() == Some(SCOPE_FRAME) {
            crate::web::turbo::stream("replace", "bots_actions", &Actions { v: ctx, select_mode, can_merge, can_split }.render()?)
        } else { String::new() };
        let body = IndexView {
            v: ctx, preferences: &preferences, bot_updates: &bot_updates,
            // User#global_pnl_snapshot with nothing computed: waiting as soon as any bot has an accepted order.
            // And while the account's history waits on a bot's marked chart (@global_pnl_history_loading).
            pnl_pass: pnl_loading || history_loading, pnl_loading,
            filters, select_mode, can_merge, can_split, stream, connected: json(&serde_json::json!(connected)), names: json(&Value::Object(names)),
            no_partner: literal("bot.merge.no_partner", "quote"), no_shared_exchange: literal("bot.merge.no_shared_exchange", "quote"),
            other_exchange: literal("bot.merge.other_exchange", "exchange"),
            refresh: (!cold.is_empty()).then(|| json(&serde_json::json!({ "bot_ids": cold }))),
            tiles, headline,
        }.render()?;
        Ok((shell, Listing::Body(body)))
    }).await;
    // One bot with a stored number this build does not read refuses the list, as any refused bot does.
    let (shell, listing) = match listed { Ok(listed) => listed, Err(error) => return layout::or_refused(&ctx, error) };
    let body = match listing {
        Listing::Body(body) => body,
        Listing::Empty => {
            // The first bots page after a sign-in with no bots opens the new-bot wizard over hidden chrome, and every wizard
            // begun before it starts over (`session.delete(:auto_open_bot_wizard)`, then `session[:bot_config] = {}`).
            let opening = {
                let mut session = ctx.session.lock();
                let opening = std::mem::take(&mut session.auto_open_bot_wizard);
                if opening { session.bot_config = Some(serde_json::json!({})); }
                opening
            };
            let (preferences, bot_updates) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id));
            let body = EmptyView { v: &ctx, preferences: &preferences, bot_updates: &bot_updates }.render()?;
            if opening {
                let src = format!("{}?auto_open=true", ctx.path("/bots/dca_single_assets/pick_exchange/new"));
                return shell::application_opening(&ctx, &ctx.csrf_token(), &user, &shell, Page { status: StatusCode::OK, body, flash_now: Vec::new() }, src);
            }
            body
        }
    };
    shell::application(&ctx, &ctx.csrf_token(), &user, &shell, Page { status: StatusCode::OK, body, flash_now: Vec::new() })
}

/// Bots::ReordersController#update: the dashboard's drag. The dragged ids (`params[:ids]`: query over body, JSON or form) as
/// `Array(params[:ids]).map(&:to_i).uniq`, the user's own only, refill the slots those bots hold in the user's whole list
/// (deleted and archived bots included), and the list is renumbered 1..n where a position differs. Only `position` is
/// written: no `updated_at`, no log, no broadcast, no wake, and no eligibility guard (no rule reads `position`).
pub async fn reorder(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: axum::http::HeaderMap) -> Result<Response, WebError> {
    use axum::response::IntoResponse;
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated_in_format(&ctx, &headers)) };
    // `params[:ids]`: the query over the body (JSON or form), as Rails merges them.
    let Ok(params) = bot::action_params::ActionParams::parse_rails(&ctx.params) else { return Ok((StatusCode::BAD_REQUEST, "Bad Request\n").into_response()) };
    let sent: Vec<i64> = match params.value().get("ids") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items.iter().map(super::wizard::to_i).collect(),
        Some(other) => vec![super::wizard::to_i(other)],
    };
    app.db(move |c| {
        let tx = crate::engine::model::immediate(c)?;
        let mut bots: Vec<(i64, i64)> = tx.prepare("SELECT id, position FROM bots WHERE user_id = ?1 ORDER BY position, id")?
            .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        let mut dragged: Vec<(i64, i64)> = vec![];
        for id in sent {
            if let Some(bot) = bots.iter().find(|bot| bot.0 == id) { if !dragged.contains(bot) { dragged.push(*bot); } }
        }
        if dragged.is_empty() { return Ok(()); }
        let slots: Vec<usize> = (0..bots.len()).filter(|&i| dragged.contains(&bots[i])).collect();
        for (slot, bot) in slots.into_iter().zip(dragged) { bots[slot] = bot; }
        for (index, (id, position)) in bots.iter().enumerate() {
            let wanted = index as i64 + 1;
            if *position != wanted { tx.execute("UPDATE bots SET position = ?1 WHERE id = ?2", [wanted, *id])?; }
        }
        tx.commit()?;
        Ok(())
    }).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
