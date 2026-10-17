//! The DCA wizard's asset step (Bots::DcaSingleAssets::PickBuyableAssetsController): the basket is collected here, one asset
//! at a time, and leaves through `advance`. The URL stays in the single namespace; once anything is chosen the later steps
//! are the multi namespace's. Every GET but `new`'s `||= {}` leaves the session alone: Turbo prefetches on hover.
use super::{chips, chosen_ids, current_variant, exchange_id, expired, first_incomplete_after, progress, reset_keys_but_exchange, sentence, stock_notice, BotType, Chip,
            SentenceOf, Step, StepOrder, Variant, EXCHANGE_KEY};
use crate::web::bot::search::{self, AssetPage, Basket, Item, Side};
use crate::web::layout::{self, Ctx, Page};
use crate::web::shell::{self, Shell};
use crate::web::{auth, flash, i18n, App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;
use serde_json::{json, Value};

/// Bots::DcaMultiAsset::MAX_ASSETS.
pub const MAX_ASSETS: usize = 100;
const NEW: &str = "/bots/dca_single_assets/pick_buyable_asset/new";
const FIELD: &str = "bots_dca_single_asset[base_asset_id]";

/// The controller's step_path: the basket's steps are the multi namespace's; an empty basket's exchange and key steps come
/// before any asset, in the single one.
fn step_path(step: Step, basket_empty: bool) -> &'static str {
    match (step, basket_empty) {
        (Step::Currencies | Step::Assets, _) => NEW,
        (Step::Exchange, true) => "/bots/dca_single_assets/pick_exchange/new",
        (Step::Api, true) => "/bots/dca_single_assets/add_api_key/new",
        (Step::Exchange, false) => "/bots/dca_multi_assets/pick_exchange/new",
        (Step::Api, false) => "/bots/dca_multi_assets/add_api_key/new",
        (Step::Spendable, _) => "/bots/dca_multi_assets/pick_spendable_asset/new",
    }
}

/// What the step reads from the session: the chosen Assets in pick order, and the venue the search is narrowed to (none on an
/// asset-first fresh start, where a venue left behind is neither shown nor searched against).
struct Chosen { chips: Vec<Chip>, ids: Vec<i64>, variant: Variant, venue: Option<i64> }

fn chosen(c: &Connection, config: Option<&Value>) -> Result<Chosen, WebError> {
    let chips = chips(c, &chosen_ids(config))?;
    let variant = current_variant(config);
    let fresh_start = variant == Variant::AssetFirst && chips.is_empty();
    let ids = chips.iter().map(|chip| chip.id).collect();
    Ok(Chosen { chips, ids, variant, venue: if fresh_start { None } else { exchange_id(config) } })
}

impl Chosen {
    /// `search_scope`: before the first pick the blank one-asset bot, then the basket. Neither sees the session's quote.
    /// ponytail: the blank bot's scope is read as an empty basket's, which leaves out a retired venue Rails' one-pair scope still
    /// lists (Exchange.available, not .tradeable) and a chosen venue no longer available; no install of this build has either.
    fn scope(&self) -> Basket { Basket { exchange_id: self.venue, quote_asset_id: None, base_asset_ids: self.ids.clone() } }
}

/// `eligible_exchanges_for(basket)`: the venues carrying every chosen asset, narrowed to the chosen venue.
fn eligible(c: &Connection, basket: &Basket) -> Result<Vec<crate::web::bot::Exchange>, WebError> {
    let venues = search::available_exchanges(c, &Basket { exchange_id: None, ..basket.clone() }, None)?;
    Ok(venues.into_iter().filter(|venue| basket.exchange_id.is_none_or(|id| id == venue.id)).collect())
}

#[derive(Template)]
#[template(path = "wizard/pick_buyable_asset.html")]
struct New<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    close_url: String,
    progress: String,
    sentence: String,
    chosen: Vec<Chip>,
    basket_open: bool,
    remove_url: String,
    remove_title: String,
    no_common_exchange: Option<String>,
    max_reached: Option<String>,
    search_url: String,
    create_url: String,
    syncing: Option<String>,
    field_name: &'static str,
    items: Vec<Item>,
    next: Option<(String, String)>,
    notice: String,
}

/// `prepare_step` and the page: the whole step, or (`render_asset_page`, a lazy frame asking for `offset`) one page of results.
fn page(c: &Connection, app: &App, ctx: &Ctx, admin: bool, config: Option<&Value>, partial_allowed: bool) -> Result<(String, bool), WebError> {
    let chosen = chosen(c, config)?;
    let scope = chosen.scope();
    let params = &ctx.params;
    let (rows, offset, next) = if chosen.chips.len() >= MAX_ASSETS {
        // The lazy renderer expects an offset even though the capped list has no search page.
        (vec![], 0, None)
    } else {
        let offset = params.query("offset").map_or(0, search::to_i);
        let all = search::filtered_rows(c, &scope, Side::Base, params.query("query"))?;
        let next_offset = offset.saturating_add(search::PAGE_SIZE as i64);
        let next = (all.len() as i64 > next_offset).then(|| {
            let query = params.fullpath.split_once('?').map_or("", |(_, query)| query);
            (format!("assets-page-{next_offset}"), format!("{}?{}", params.full_path, search::next_page_query(query, next_offset)))
        });
        let mut rows = search::page_of(all, offset);
        search::attach_exchanges(c, &scope, Side::Base, &mut rows)?;
        (rows, offset, next)
    };
    let items = search::items(rows)?;
    if partial_allowed && offset > 0 {
        return Ok((AssetPage { frame_id: format!("assets-page-{offset}"), field_name: FIELD.into(), items, next }.render()?, true));
    }
    let csrf = ctx.csrf_token();
    let no_common_exchange = (!chosen.chips.is_empty() && eligible(c, &scope)?.is_empty()).then(|| ctx.t("bot.dca_multi_asset.no_common_exchange"));
    let syncing = (items.is_empty() && chosen.chips.is_empty() && chosen.venue.is_some() && params.query("query").is_none_or(crate::ruby::blank))
        .then(|| ctx.t("bot.setup.assets_syncing"));
    let order = StepOrder::new(BotType::Single, chosen.variant);
    let exchange = super::exchange_name(c, chosen.venue)?;
    let sentence = sentence(ctx, &csrf, SentenceOf { order: &order, step: Step::Currencies, multi: !chosen.chips.is_empty(), chips: chips(c, &chosen.ids)?, exchange })?;
    let body = New {
        v: ctx, csrf: &csrf, close_url: super::close_url(ctx), progress: progress(ctx, &order, Step::Currencies)?, sentence,
        basket_open: params.query("basket") == Some("open"),
        remove_url: ctx.path("/bots/dca_single_assets/pick_buyable_asset/remove"), remove_title: ctx.t("bot.dca_multi_asset.remove_asset"),
        no_common_exchange,
        max_reached: (chosen.chips.len() >= MAX_ASSETS).then(|| i18n::t(ctx.locale, "bot.dca_multi_asset.max_assets_reached", &[("max", i18n::Arg::Text("100"))])),
        search_url: ctx.path(NEW), create_url: ctx.path("/bots/dca_single_assets/pick_buyable_asset"), syncing, field_name: FIELD, items, next,
        notice: stock_notice(c, app, ctx, admin, chosen.venue.is_some())?,
        chosen: chosen.chips,
    }.render()?;
    Ok((body, false))
}

/// The step page, through the layout (or bare, for a lazy page of results).
async fn render(app: &App, ctx: &Ctx, status: StatusCode, partial_allowed: bool) -> Result<Response, WebError> {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("the asset step without a user".into()))?;
    let config = ctx.session.lock().bot_config.clone();
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let ((body, partial), shell) = app.db(move |c| Ok((page(c, &inner, &view, owner.admin, config.as_ref(), partial_allowed)?, Shell::load(c, &inner, &owner)?))).await?;
    if partial { return Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()) }
    shell::application(ctx, &ctx.csrf_token(), &user, &shell, Page { status, body, flash_now: vec![] })
}

/// GET .../pick_buyable_asset/new: the one wizard GET that writes (`session[:bot_config] ||= {}`). An earlier step that is not
/// complete (a stale page, a direct URL) sends the browser back to it.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.get_or_insert_with(|| json!({})).clone();
    let back = app.db(move |c| {
        let order = StepOrder::new(BotType::Single, current_variant(Some(&config)));
        let Some(step) = super::prerequisite(c, user.id, Some(&config), &order, Step::Currencies)? else { return Ok(None) };
        Ok(Some(step_path(step, chips(c, &chosen_ids(Some(&config)))?.is_empty())))
    }).await?;
    if let Some(path) = back { return Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path))) }
    render(&app, &ctx, StatusCode::OK, true).await
}

/// `params.require(:bots_dca_single_asset)`: no such field at all is ActionController::ParameterMissing.
fn asset_param(ctx: &Ctx) -> Option<i64> {
    ctx.params.form(FIELD).or_else(|| ctx.params.query(FIELD)).map(search::to_i)
}

/// `write_ids`: every wizard key but the exchange goes, then the list is written (an empty one is not).
fn write_ids(config: &mut Value, ids: &[i64]) {
    if !config.is_object() { *config = json!({}); }
    reset_keys_but_exchange(config);
    if !ids.is_empty() {
        let settings = &mut config["settings"];
        if settings.is_null() { *settings = json!({}); }
        settings["base_asset_ids"] = json!(ids);
    }
}

/// POST .../pick_buyable_asset: adds one asset and stays. Only an asset the current search scope offers may join; a
/// duplicate and the hundred-and-first are ignored. A first pick needs no session (`||= {}`).
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let Some(id) = asset_param(&ctx) else { return Ok(layout::bad_request()) };
    let config = ctx.session.lock().bot_config.get_or_insert_with(|| json!({})).clone();
    if id == 0 { return render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, false).await }
    let session = ctx.session.clone();
    app.db(move |c| {
        let chosen = chosen(c, Some(&config))?;
        if !chosen.ids.contains(&id) && chosen.ids.len() < MAX_ASSETS && search::offers(c, &chosen.scope(), Side::Base, id)? {
            let mut data = session.lock();
            let config = data.bot_config.get_or_insert_with(|| json!({}));
            if chosen.variant == Variant::AssetFirst && chosen.ids.is_empty() {
                if let Some(entries) = config.as_object_mut() { entries.remove(EXCHANGE_KEY[0]); }
            }
            write_ids(config, &[chosen.ids.as_slice(), &[id]].concat());
        }
        Ok(())
    }).await?;
    Ok(layout::redirect(StatusCode::FOUND, &ctx.path(NEW)))
}

/// POST .../pick_buyable_asset/remove: the basket table comes back open, the user was working in it.
pub async fn remove(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let Some(id) = asset_param(&ctx) else { return Ok(layout::bad_request()) };
    let config = ctx.session.lock().bot_config.clone();
    let ids = app.db(move |c| Ok(chosen(c, config.as_ref())?.ids)).await?;
    let kept: Vec<i64> = ids.into_iter().filter(|chosen| *chosen != id).collect();
    write_ids(ctx.session.lock().bot_config.get_or_insert_with(|| json!({})), &kept);
    Ok(layout::redirect(StatusCode::FOUND, &format!("{}?basket=open", ctx.path(NEW))))
}

/// Where `advance` goes, decided with the session as it stood.
enum Exit { To(&'static str), NoCommonExchange, Broker(Option<(i64, Option<i64>)>, &'static str) }

/// POST .../pick_buyable_asset/advance: the one way out of the step, committing the basket in its canonical shape first. The
/// exchange chip (`to=exchange`) goes to the exchange step; Next to the first step of the order still missing input, a stock
/// basket picked asset-first by way of its broker.
pub async fn advance(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let to_exchange = ctx.params.form("to").or_else(|| ctx.params.query("to")) == Some("exchange");
    let session = ctx.session.clone();
    let exit = app.db(move |c| {
        let config = session.lock().bot_config.clone();
        let chosen = chosen(c, config.as_ref())?;
        if to_exchange {
            write_ids(session.lock().bot_config.get_or_insert_with(|| json!({})), &chosen.ids);
            return Ok(Exit::To(step_path(Step::Exchange, chosen.ids.is_empty())));
        }
        if chosen.ids.is_empty() { return Ok(Exit::To(NEW)) }
        // `basket`: the session's venue as it is, fresh start or not.
        let basket = Basket { exchange_id: exchange_id(config.as_ref()), quote_asset_id: None, base_asset_ids: chosen.ids.clone() };
        if eligible(c, &basket)?.is_empty() { return Ok(Exit::NoCommonExchange) }
        let mut config = config.unwrap_or_else(|| json!({}));
        write_ids(&mut config, &chosen.ids);
        session.lock().bot_config = Some(config.clone());
        if chosen.variant == Variant::AssetFirst && stock_basket(c, &chosen.ids)? {
            let usd = super::usd_asset(c)?;
            let brokers = stock_brokers(c, &basket, usd)?;
            if !basket.exchange_id.is_some_and(|id| brokers.contains(&id)) {
                return Ok(match brokers.as_slice() {
                    [one] => Exit::Broker(Some((*one, usd)), "/bots/dca_multi_assets/add_api_key/new"),
                    [] => Exit::Broker(None, NEW),
                    _ => Exit::Broker(None, "/bots/dca_multi_assets/pick_stock_broker/new"),
                });
            }
        }
        let order = StepOrder::new(BotType::Multi, chosen.variant);
        Ok(Exit::To(step_path(first_incomplete_after(c, user.id, Some(&config), &order, Step::Assets)?, false)))
    }).await?;
    let path = match exit {
        Exit::To(path) => path,
        Exit::NoCommonExchange => {
            flash::set(&ctx.session, flash::ALERT, i18n::text(ctx.locale, "bot.dca_multi_asset.no_common_exchange", &[]));
            NEW
        }
        Exit::Broker(chosen, path) => {
            let mut data = ctx.session.lock();
            let config = data.bot_config.get_or_insert_with(|| json!({}));
            match chosen {
                // finalize_stock_broker!: the one broker and the USD quote default.
                Some((broker, usd)) => {
                    config["exchange_id"] = json!(broker);
                    if let Some(usd) = usd {
                        let settings = &mut config["settings"];
                        if !settings.is_object() { *settings = json!({}); }
                        settings["quote_asset_id"] = json!(usd);
                    }
                }
                None => if let Some(entries) = config.as_object_mut() { entries.remove(EXCHANGE_KEY[0]); },
            }
            drop(data);
            // The routing is silent when no broker qualifies; the user needs the reason they stayed.
            if path == NEW { flash::set(&ctx.session, flash::ALERT, i18n::text(ctx.locale, "bot.dca_multi_asset.no_common_exchange", &[])); }
            path
        }
    };
    Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path)))
}

/// `chosen_assets.any? { |asset| asset.category == 'Stock' }`.
fn stock_basket(c: &Connection, ids: &[i64]) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE category = 'Stock' AND id IN (SELECT value FROM json_each(?1)))", [json!(ids).to_string()], |r| r.get(0))?)
}

/// Bots::StockBrokerRoutable#available_stock_brokers: the stock venues listing every member, quoted in USD (the quote set only
/// in memory).
pub fn stock_brokers(c: &Connection, basket: &Basket, usd: Option<i64>) -> Result<Vec<i64>, WebError> {
    let priced = Basket { quote_asset_id: basket.quote_asset_id.or(usd), ..basket.clone() };
    Ok(search::available_exchanges(c, &priced, None)?.into_iter().filter(|venue| matches!(venue.class.as_str(), "Exchanges::Alpaca" | "Exchanges::Ibkr"))
        .map(|venue| venue.id).collect())
}
