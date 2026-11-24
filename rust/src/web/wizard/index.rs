//! The index wizard (Bots::DcaIndexes::*). D-INDEX-IN-ASSETS: an index is picked on the DCA wizard's Assets step (its Index view,
//! `assets_view`, which connects market data in place when none is), posted to `pick_create`; the venue (PickExchangesController),
//! the key (keys.rs, `Namespace::Index`) and the quote (PickSpendableAssetsController), whose POST saves the bot (index_create),
//! are drawn in the sheet with the index track. Every index step sends the browser to the Index view while no market data is
//! configured or no index is chosen; the old CoinGecko and index steps' URLs go there too.
//!
//! What this engine runs is an index bot on Alpaca, Kraken or Binance on a data-api install (eligibility::admission and index_reasons);
//! the steps offer what 2.x offers, and the key and quote steps refuse, by name, an index bot this build would not run.
use super::index_create::{self as create, count_named, display_index_name, index_row};
use super::{chips, chosen_ids, exchange_id, exchange_name, expired, fresh_start, index_chosen, index_track, search_box, step_complete, At, Step,
            BASE_IDS_KEY, BASE_KEY, EXCHANGE_KEY, INDEX_VIEW, QUOTE_KEY};
use crate::engine::{eligibility, model};
use crate::web::bot::search::{self, Item};
use crate::web::layout::{self, Ctx, Page};
use crate::web::shell::{self, Shell};
use crate::web::{auth, colors, flash, i18n, App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

const PICK: &str = "/bots/dca_indexes/pick_index";
const EXCHANGE: &str = "/bots/dca_indexes/pick_exchange";
const KEY_NEW: &str = "/bots/dca_indexes/add_api_key/new";
const QUOTE: &str = "/bots/dca_indexes/pick_spendable_asset";
const QUOTE_NEW: &str = "/bots/dca_indexes/pick_spendable_asset/new";
const EXCHANGE_FIELD: &str = "bots_dca_index[exchange_id]";
const QUOTE_FIELD: &str = "bots_dca_index[quote_asset_id]";

/// Index::ExchangeAvailability::DISPLAY_COINS_COUNT and MINIMUM_SUPPORTED_COINS; the quote rows show six.
const TILE_PILLS: usize = 5;
const ROW_PILLS: usize = 6;
const MINIMUM_SUPPORTED_COINS: i64 = 3;

fn settings_of(config: Option<&Value>) -> Map<String, Value> {
    config.and_then(|config| config.get("settings")).and_then(Value::as_object).cloned().unwrap_or_default()
}

/// MarketData.configured?
fn configured(c: &Connection, app: &App) -> Result<bool, WebError> { Ok(crate::web::bots::market_data(c, app)?.1) }

/// Why this engine would not run an index bot on a venue of `class`, if it would not: a venue the engine admits index bots
/// on (eligibility::admission), on the deltabadger provider or with a provider that ranks indices (eligibility::index_data:
/// a CoinGecko key ranks Top Coins and categories; D-IDX-COINGECKO now refuses only an install with no provider).
pub fn refusal(c: &Connection, app: &App, class: &str) -> Result<Option<&'static str>, WebError> {
    if let Some(why) = eligibility::admission(Some(class), true, false) { return Ok(Some(why)) }
    if !crate::web::bots::market_data(c, app)?.0 && !eligibility::index_data(c)? { return Ok(Some("an index bot whose market data comes from CoinGecko")) }
    Ok(None)
}

/// The index a bot's settings follow, as the engine names it (Bot#index_external_id): a category's id, else Top Coins.
pub fn followed(settings: Option<&Map<String, Value>>) -> String {
    let Some(settings) = settings else { return "top-coins".to_string() };
    match (settings.get("index_type").and_then(Value::as_str), settings.get("index_category_id").and_then(Value::as_str)) {
        (Some("category"), Some(id)) if !id.trim().is_empty() => id.to_string(),
        _ => "top-coins".to_string(),
    }
}

/// `session[:bot_config] ||= {}`.
fn begin(ctx: &Ctx) {
    let mut data = ctx.session.lock();
    if matches!(data.bot_config, None | Some(Value::Null) | Some(Value::Bool(false))) { data.bot_config = Some(json!({})); }
}

/// A step page in the sheet, its track and content made on the database thread.
async fn render<F>(app: &App, ctx: &Ctx, status: StatusCode, flash_now: Vec<(&'static str, String)>, content: F) -> Result<Response, WebError>
where F: FnOnce(&Connection, &Ctx, &str) -> Result<(String, String), WebError> + Send + 'static {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("an index step without a user".into()))?;
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let (body, shell) = app.db(move |c| {
        let csrf = view.csrf_token();
        let (track, content) = content(c, &view, &csrf)?;
        Ok((super::sheet(&view, &track, &content)?, Shell::load(c, &inner, &owner)?))
    }).await?;
    shell::application(ctx, &ctx.csrf_token(), &user, &shell, Page { status, body, flash_now })
}

fn redirect(ctx: &Ctx, path: &str) -> Response { layout::redirect(StatusCode::FOUND, &ctx.path(path)) }

/// GET .../setup_coingecko/new and .../pick_index/new: the old CoinGecko and index steps (a stale tab, a bookmark). The index is
/// picked on the Assets step's Index view, which connects market data when none is.
pub async fn setup_new(Extension(ctx): Extension<Ctx>) -> Response {
    if ctx.user().is_none() { return auth::unauthenticated(&ctx) }
    redirect(&ctx, INDEX_VIEW)
}

/// POST .../setup_coingecko (SetupCoingeckosController#create), which no form posts to now: admin only; a key CoinGecko refuses
/// is answered with the alert, else it is saved, the setup runs, and the wizard (begun) goes on to the Index view.
pub async fn setup_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    use crate::web::settings::market_data as md;
    let Some(user) = ctx.user() else { return Ok(auth::unauthenticated(&ctx)) };
    if !user.admin { return Ok(md::head(StatusCode::FORBIDDEN)) }
    let key = ctx.params.form("api_key").unwrap_or_default().to_string();
    if crate::v2::blank(&key) || !md::key_valid(&app, &key).await { return md::invalid_key(&ctx) }
    md::save_key(&app, key).await?;
    begin(&ctx);
    Ok(redirect(&ctx, INDEX_VIEW))
}

pub(crate) struct Pill { pub(crate) id: i64, pub(crate) symbol: String, pub(crate) color: String }

/// One tile of the picker; `stored_name` is the `indices` row's name, for the bot menu's switch.
pub(crate) struct IndexTile { pub(crate) stored_name: Option<String>, pub(crate) index_type: &'static str,
    pub(crate) top: bool, pub(crate) name: String, pub(crate) external_id: String, pub(crate) description: String, pub(crate) summary: String,
    pub(crate) has_coins: bool, pub(crate) pills: Vec<Pill>, pub(crate) more: i64,
    /// The venue types its `available_exchanges` names (none: every venue).
    pub(crate) venues: Vec<String> }

/// Index.followable_on(exchange, quote_asset_id): the indices of which at least MINIMUM_SUPPORTED_COINS members exported for the
/// venue's class trade there at the quote; `current` (an index bot's own index) is left out, as Bots::IndicesController does.
pub(crate) struct Followable<'a> { pub exchange_id: i64, pub class: &'a str, pub quote: Option<i64>, pub current: Option<i64> }

/// `truncate(text, length: 150)`: past 150 characters, the first 147 and "...".
fn truncate(text: &str) -> String {
    if text.chars().count() <= 150 { return text.to_string() }
    format!("{}...", text.chars().take(147).collect::<String>())
}

fn ids_of(value: Option<String>) -> Vec<String> {
    match value.and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
        Some(Value::Array(ids)) => ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

/// Index.for_picker and the tiles: data-api indices first (shown only on a data-api install), then the internal Top Coins,
/// then CoinGecko categories; by weight, then market cap. `followable` narrows them to what a bot could follow.
pub(crate) fn tiles(c: &Connection, app: &App, followable: Option<&Followable>) -> Result<Vec<IndexTile>, WebError> {
    let deltabadger = crate::web::bots::market_data(c, app)?.0;
    let mut statement = c.prepare("SELECT external_id, source, name, description, top_coins, available_exchanges, id, top_coins_by_exchange FROM indices WHERE ?1 OR source != 'deltabadger' \
                                   ORDER BY CASE source WHEN 'deltabadger' THEN 0 WHEN 'internal' THEN 1 ELSE 2 END, weight DESC, market_cap DESC, id")?;
    type IndexColumns = (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, Option<String>);
    let mut rows = statement.query_map([deltabadger], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?
        .collect::<Result<Vec<IndexColumns>, _>>()?;
    if let Some(f) = followable {
        let mut s = c.prepare("SELECT external_id FROM assets WHERE external_id IS NOT NULL AND id IN (SELECT base_asset_id FROM tickers \
                               WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND quote_asset_id IS ?2)")?;
        let traded: std::collections::HashSet<String> = s.query_map(rusqlite::params![f.exchange_id, f.quote], |r| r.get(0))?.collect::<Result<_, _>>()?;
        rows.retain(|row| {
            let index = create::IndexRow { source: None, top_coins: ids_of(row.4.clone()), available_exchanges: Map::new(),
                by_exchange: row.7.as_deref().and_then(|text| serde_json::from_str::<Value>(text).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default() };
            Some(row.6) != f.current && index.top_coins_for_exchange(f.class).iter().filter(|coin| traded.contains(*coin)).count() as i64 >= MINIMUM_SUPPORTED_COINS
        });
    }
    // `Asset.where(external_id: ...).index_by(&:external_id)`: the last row of an id wins.
    let wanted: Vec<String> = rows.iter().flat_map(|row| ids_of(row.4.clone())).collect();
    let mut assets: std::collections::HashMap<String, (i64, String, Option<String>)> = Default::default();
    let mut find = c.prepare("SELECT external_id, id, symbol, color FROM assets WHERE external_id IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
    for row in find.query_map([json!(wanted).to_string()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)))? {
        let (external, id, symbol, color) = row?;
        assets.insert(external, (id, symbol.unwrap_or_default(), color));
    }
    rows.into_iter().map(|(external_id, source, name, description, top_coins, available, _, _)| {
        let external_id = external_id.unwrap_or_default();
        let coins = ids_of(top_coins);
        let mut pills = vec![];
        for coin in &coins {
            if pills.len() >= TILE_PILLS { break }
            let Some((id, symbol, color)) = assets.get(coin) else { continue };
            pills.push(Pill { id: *id, symbol: symbol.clone(), color: colors::pill_color(color.as_deref()).ok_or_else(|| WebError::Config("an asset colour 2.x cannot read".into()))? });
        }
        let available = available.and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        let most = available.values().filter_map(|count| count.as_i64().or_else(|| count.as_f64().map(|f| f as i64))).max().unwrap_or(0);
        let top = source.as_deref() == Some("internal");
        let description = description.unwrap_or_default();
        Ok(IndexTile {
            stored_name: name.clone(), index_type: if top { "top" } else { "category" }, top,
            name: count_named(Some(&external_id)).map_or_else(|| name.unwrap_or_default(), |(_, named)| named.to_string()),
            summary: truncate(&description), description, has_coins: !coins.is_empty(), more: most - pills.len() as i64, pills, external_id,
            venues: available.keys().cloned().collect(),
        })
    }).collect()
}

/// The Index view's list: `tiles` narrowed by the venue in force on the Assets step (none on a fresh start): Top Coins and the
/// indices naming no venue always, a category when it names the venue's type; nothing on a venue `refusal` refuses.
fn offered(c: &Connection, app: &App, config: Option<&Value>) -> Result<Vec<IndexTile>, WebError> {
    let all = tiles(c, app, None)?;
    let Some(venue) = super::keys::venue(c, super::assets::venue_in_force(c, config)?)? else { return Ok(all) };
    if refusal(c, app, &venue.class)?.is_some() { return Ok(vec![]) }
    Ok(all.into_iter().filter(|tile| tile.top || tile.venues.is_empty() || tile.venues.contains(&venue.class)).collect())
}

#[derive(Template)]
#[template(path = "wizard/index/assets.html")]
struct AssetsView<'a> { v: &'a Ctx, switch: String, error: Option<String>, configured: bool, admin: bool, widget: Option<String>, providers: String,
    action: String, csrf: &'a str, tiles: Vec<IndexTile> }

/// D-INDEX-IN-ASSETS: the Assets step's Index view, as the sheet: the Asset / Index switch, then the index list (market data
/// configured), Settings' market-data widget (an admin), or who to ask. The track is the DCA one until an index is chosen.
pub fn assets_view(c: &Connection, app: &App, ctx: &Ctx, config: Option<&Value>, error: Option<String>) -> Result<String, WebError> {
    let csrf = ctx.csrf_token();
    let admin = ctx.user().is_some_and(|user| user.admin);
    let configured = configured(c, app)?;
    // D-INDEX-CONNECT-IN-VIEW: no market data: the providers as the exchange step's tiles, each opening its connect card.
    let provider = ctx.params.query("provider").filter(|p| matches!(*p, "coingecko" | "deltabadger"));
    let widget = match provider { Some(provider) if !configured && admin => Some(crate::web::settings::keys::market_data(c, app, ctx, &csrf, false, None, Some(provider))?), _ => None };
    let providers = crate::web::settings::sections::market_data_providers(ctx, "modal_content", |key| ctx.path(&format!("{INDEX_VIEW}&provider={key}")));
    let tiles = if configured { offered(c, app, config)? } else { vec![] };
    let content = AssetsView { v: ctx, switch: super::kind_switch(ctx, true)?, error, configured, admin, widget, providers, action: ctx.path(PICK), csrf: &csrf, tiles }.render()?;
    let venue = super::assets::venue_in_force(c, config)?;
    let track = if index_chosen(config) {
        index_track(ctx, &csrf, config, At::Assets, &index_name(c, config, ctx.locale)?, exchange_name(c, venue)?)?
    } else {
        let order = super::StepOrder::new(super::BotType::Single, super::current_variant(config));
        let ids = chosen_ids(config);
        super::steer(ctx, &csrf, super::SentenceOf { order: &order, step: Step::Currencies, multi: !ids.is_empty(), chips: chips(c, &ids)?, exchange: exchange_name(c, venue)? }, true)?.track
    };
    super::assets_sheet(ctx, &track, &content)
}

/// POST .../pick_index: an index the Index view offers (Top Coins, or a category with its id) joins the session's settings in place
/// of a basket (D-INDEX-OR-BASKET); one it does not offer re-renders the view (422) with the reason.
pub async fn pick_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let param = |name: &str| ctx.params.form(name).or_else(|| ctx.params.query(name)).map(str::to_string);
    let (index_type, category, name) = (param("index_type"), param("index_category_id").filter(|id| !crate::v2::blank(id)), param("index_name"));
    let config = ctx.session.lock().bot_config.clone();
    let (inner, held, wanted, id) = (app.clone(), config.clone(), index_type.clone(), category.clone());
    let (configured, offered, fresh) = app.db(move |c| {
        let fresh = fresh_start(held.as_ref(), chips(c, &chosen_ids(held.as_ref()))?.is_empty());
        if !configured(c, &inner)? { return Ok((false, false, fresh)) }
        let offered = offered(c, &inner, held.as_ref())?.iter().any(|tile| match (wanted.as_deref(), id.as_deref()) {
            (Some("top"), _) => tile.top,
            (Some("category"), Some(id)) => !tile.top && tile.external_id == id,
            _ => false,
        });
        Ok((true, offered, fresh))
    }).await?;
    if !configured { return Ok(redirect(&ctx, INDEX_VIEW)) }
    if !offered {
        let (inner, view, error) = (app.clone(), ctx.clone(), ctx.t("bot.dca_index.setup.pick_index.error"));
        let (body, shell) = app.db(move |c| Ok((assets_view(c, &inner, &view, config.as_ref(), Some(error))?, Shell::load(c, &inner, &user)?))).await?;
        return shell::application(&ctx, &ctx.csrf_token(), ctx.user().expect("checked above"), &shell, Page { status: StatusCode::UNPROCESSABLE_ENTITY, body, flash_now: vec![] });
    }
    {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        for key in [BASE_IDS_KEY, BASE_KEY, QUOTE_KEY] { super::delete_path(config, key); }
        // Asset first with nothing chosen before: a venue left behind goes, as the first asset drops it.
        if fresh { if let Some(entries) = config.as_object_mut() { entries.remove(EXCHANGE_KEY[0]); } }
        if !config["settings"].is_object() { config["settings"] = json!({}); }
        let settings = config["settings"].as_object_mut().expect("an object, made one above");
        match (index_type.as_deref(), category) {
            (Some("category"), Some(id)) => {
                let prefix = count_named(Some(&id)).map(|(prefix, _)| prefix);
                settings.insert("index_type".into(), json!("category"));
                settings.insert("index_category_id".into(), json!(id));
                settings.insert("index_name".into(), json!(name));
                settings.insert("index_name_prefix".into(), json!(prefix));
            }
            _ => {
                for key in ["index_type", "index_category_id", "index_name", "index_name_prefix"] { settings.insert(key.into(), Value::Null); }
                settings.insert("index_type".into(), json!("top"));
            }
        }
    }
    Ok(redirect(&ctx, KEY_NEW))
}

/// The before_actions of every index step: market data, then an index chosen; else the Index view, which asks for either.
pub fn gate(c: &Connection, app: &App, config: Option<&Value>) -> Result<Option<&'static str>, WebError> {
    Ok((!configured(c, app)? || !index_chosen(config)).then_some(INDEX_VIEW))
}

/// The index's name as the bot built from the session shows it, after_initialize's coin count included.
pub(super) fn index_name(c: &Connection, config: Option<&Value>, locale: &str) -> Result<String, WebError> {
    let mut settings = settings_of(config);
    let bounded = create::bounded(index_row(c, &settings)?.as_ref());
    if matches!(settings.get("num_coins"), None | Some(Value::Null)) { settings.insert("num_coins".into(), json!(bounded.map_or(10, |size| size.min(10)))); }
    Ok(display_index_name(&settings, bounded, locale))
}

/// `exchange_search_results_for_index_bot`: the venues a category index is available on (every available venue when it names
/// none, or for Top Coins), ordered or searched as Bots::Searchable does.
fn exchange_page(c: &Connection, ctx: &Ctx, csrf: &str, config: Option<&Value>) -> Result<(String, String), WebError> {
    let settings = settings_of(config);
    let category = settings.get("index_category_id").and_then(Value::as_str).filter(|id| !crate::v2::blank(id));
    let types: Option<Vec<String>> = match (settings.get("index_type").and_then(Value::as_str), category) {
        (Some("category"), Some(id)) => {
            let available: Option<Option<String>> = c.query_row("SELECT available_exchanges FROM indices WHERE external_id = ?1 ORDER BY id LIMIT 1", [id], |r| r.get(0)).optional()?;
            let keys: Vec<String> = available.flatten().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
            (!keys.is_empty()).then_some(keys)
        }
        _ => None,
    };
    let mut statement = c.prepare("SELECT id, name, type FROM exchanges WHERE available = 1 AND (?1 IS NULL OR type IN (SELECT value FROM json_each(?1))) ORDER BY id")?;
    let venues = statement.query_map([types.map(|t| json!(t).to_string())], |r| Ok(crate::web::bot::Exchange {
        id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), class: r.get::<_, Option<String>>(2)?.unwrap_or_default(), maker_fee: None, number: None,
    }))?.collect::<Result<Vec<_>, _>>()?;
    let table = super::exchanges::Table { action: ctx.path(EXCHANGE), csrf: csrf.to_string(), param_name: EXCHANGE_FIELD.into(), venues: vec![],
                                          unavailable_note: ctx.t("bot.hyperliquid_trading_unavailable") };
    let content = super::exchanges::venue_pane(ctx, table, venues, &super::exchanges::connected(c, ctx.user().map_or(0, |user| user.id), &[0])?)?;
    Ok((index_track(ctx, csrf, config, At::Exchange, &index_name(c, config, ctx.locale)?, None)?, content))
}

async fn exchange_render(app: &App, ctx: &Ctx, status: StatusCode) -> Result<Response, WebError> {
    let config = ctx.session.lock().bot_config.clone();
    render(app, ctx, status, vec![], move |c, ctx, csrf| exchange_page(c, ctx, csrf, config.as_ref())).await
}

/// GET .../pick_exchange/new.
pub async fn exchange_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let (inner, config) = (app.clone(), ctx.session.lock().bot_config.clone());
    if let Some(path) = app.db(move |c| gate(c, &inner, config.as_ref())).await? { return Ok(redirect(&ctx, path)) }
    begin(&ctx);
    exchange_render(&app, &ctx, StatusCode::OK).await
}

/// POST .../pick_exchange: the venue joins the session as the form sent it; a blank one re-renders the step (422).
pub async fn exchange_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let (inner, config) = (app.clone(), ctx.session.lock().bot_config.clone());
    if let Some(path) = app.db(move |c| gate(c, &inner, config.as_ref())).await? { return Ok(redirect(&ctx, path)) }
    let Some(exchange) = ctx.params.form(EXCHANGE_FIELD).or_else(|| ctx.params.query(EXCHANGE_FIELD)).map(str::to_string) else { return Ok(layout::bad_request()) };
    if crate::v2::blank(&exchange) { return exchange_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY).await }
    let (view, user, id) = (ctx.clone(), ctx.user().map_or(0, |u| u.id), super::to_i(&json!(exchange)));
    let Some(picked) = app.db(move |c| Ok(super::picked(c, &view, user, id)?)).await? else { return Ok(layout::missing()) };
    {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        config["exchange_id"] = json!(exchange);
        super::keep_pick(config, picked);
    }
    Ok(redirect(&ctx, KEY_NEW))
}

struct QuoteRow { item: Item, pills: Vec<Pill>, more: i64 }

#[derive(Template)]
#[template(path = "wizard/index/pick_spendable_asset.html")]
struct QuoteStep { search: String, create_url: String, csrf: String, rows: Vec<QuoteRow> }

/// The quote step: Bots::DcaIndex#available_assets_for_current_settings (the quotes in which the venue lists at least three of
/// the index's members; Top Coins any three) ranked or searched, one page of them, each with the index's members it pairs
/// with on the venue (top_base_assets_by_quote_for_current_setup), by market-cap rank.
/// `base_asset_ids if coin_ids.present?`: a category index's members exported for the venue's class, as asset ids, narrowing
/// only when any is an Asset (JSON text for `?2` of `available_quotes`).
fn narrowed(c: &Connection, settings: &Map<String, Value>, class: &str) -> Result<Option<String>, WebError> {
    let category = settings.get("index_type").and_then(Value::as_str) == Some("category")
        && settings.get("index_category_id").and_then(Value::as_str).is_some_and(|id| !crate::v2::blank(id));
    if !category { return Ok(None); }
    let coins = index_row(c, settings)?.map(|index| index.top_coins_for_exchange(class)).unwrap_or_default();
    let ids: Vec<i64> = if coins.is_empty() { vec![] } else {
        let mut s = c.prepare("SELECT id FROM assets WHERE external_id IN (SELECT value FROM json_each(?1))")?;
        let ids = s.query_map([json!(coins).to_string()], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    Ok((!ids.is_empty()).then(|| json!(ids).to_string()))
}

/// Bots::DcaIndex#available_assets_for_current_settings(asset_type: :quote_asset): the quotes MINIMUM_SUPPORTED_COINS of
/// the index's members trade against on the venue (`?1` the venue's id, `?2` `narrowed`).
fn available_quotes() -> String {
    format!("id IN (SELECT quote_asset_id FROM tickers WHERE available = 1 AND trading_enabled = 1 \
             AND exchange_id IN (SELECT id FROM exchanges WHERE (?1 IS NULL AND available = 1) OR id = ?1) \
             AND (?2 IS NULL OR base_asset_id IN (SELECT value FROM json_each(?2))) GROUP BY quote_asset_id HAVING COUNT(*) >= {MINIMUM_SUPPORTED_COINS})")
}

/// The quote step's offer, `find_by(symbol:)` (BotApi::Bots::CreateIndex): the first such asset of that symbol.
pub fn quote_asset(c: &Connection, exchange: i64, class: &str, settings: &Map<String, Value>, symbol: &str) -> Result<Option<i64>, WebError> {
    let narrowed = narrowed(c, settings, class)?;
    Ok(c.query_row(&format!("SELECT id FROM assets WHERE symbol = ?3 AND {} ORDER BY id LIMIT 1", available_quotes()),
                   rusqlite::params![exchange, narrowed, symbol], |r| r.get(0)).optional()?)
}

fn quote_page(c: &Connection, ctx: &Ctx, csrf: &str, config: Option<&Value>) -> Result<(String, String), WebError> {
    let settings = settings_of(config);
    let venue = super::keys::venue(c, exchange_id(config))?;
    let index = index_row(c, &settings)?;
    let members = |venue: &super::keys::Venue| index.as_ref().map(|index| index.top_coins_for_exchange(&venue.class)).unwrap_or_default();
    let narrowed = match &venue { Some(venue) => narrowed(c, &settings, &venue.class)?, None => None };
    let available = available_quotes();
    let all = search::ranked_rows(c, &available, vec![venue.as_ref().map(|v| v.id).into(), narrowed.into()], ctx.params.query("query"))?;
    let page = search::page_of(all, ctx.params.query("offset").map_or(0, search::to_i));
    // top_base_assets_by_quote_for_current_setup.
    let mut by_quote: Vec<(i64, Pill)> = vec![];
    if let Some(venue) = &venue {
        let coins = members(venue);
        if index.is_some() && !coins.is_empty() {
            let mut s = c.prepare("SELECT t.quote_asset_id, a.id, a.symbol, a.color FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                                   WHERE t.available = 1 AND t.trading_enabled = 1 AND t.exchange_id = ?1 AND a.external_id IN (SELECT value FROM json_each(?2)) \
                                   ORDER BY (SELECT MIN(CAST(j.key AS INTEGER)) FROM json_each(?2) j WHERE j.value = a.external_id), t.id")?;
            // The index's own order (largest first), ties in ticker order: market_cap_rank is a crypto ranking, and stocks have none.
            for row in s.query_map(rusqlite::params![venue.id, json!(coins).to_string()], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)))? {
                let (quote, id, symbol, color) = row?;
                let Some(quote) = quote else { continue };
                by_quote.push((quote, Pill { id, symbol: symbol.unwrap_or_default(), color: colors::pill_color(color.as_deref()).ok_or_else(|| WebError::Config("an asset colour 2.x cannot read".into()))? }));
            }
        }
    }
    let mut rows = vec![];
    for item in search::items(page)? {
        let mut all: Vec<Pill> = vec![];
        by_quote.retain_mut(|(quote, pill)| if *quote == item.id { all.push(Pill { id: pill.id, symbol: std::mem::take(&mut pill.symbol), color: std::mem::take(&mut pill.color) }); false } else { true });
        let more = all.len() as i64 - ROW_PILLS as i64;
        all.truncate(ROW_PILLS);
        rows.push(QuoteRow { item, pills: all, more });
    }
    let content = QuoteStep { search: search_box("asset-picker", &ctx.path(QUOTE_NEW), "USD")?, create_url: ctx.path(&format!("{QUOTE}{}", super::bound_query(config))), csrf: csrf.to_string(), rows }.render()?;
    Ok((index_track(ctx, csrf, config, At::Quote, &index_name(c, config, ctx.locale)?, venue.map(|venue| venue.name))?, content))
}

async fn quote_render(app: &App, ctx: &Ctx, status: StatusCode, flash_now: Vec<(&'static str, String)>) -> Result<Response, WebError> {
    let config = ctx.session.lock().bot_config.clone();
    render(app, ctx, status, flash_now, move |c, ctx, csrf| quote_page(c, ctx, csrf, config.as_ref())).await
}

/// What the quote step does before its page: no market data or no index, the Index view; the key not correct, the key step; an
/// index bot this engine would not run, refused.
enum QuoteGate { To(&'static str), Refused, Open }

fn quote_gate(c: &Connection, app: &App, user: i64, config: Option<&Value>, check_key: bool) -> Result<QuoteGate, WebError> {
    if let Some(path) = gate(c, app, config)? { return Ok(QuoteGate::To(path)) }
    if check_key && !step_complete(c, user, config, Step::Api)? { return Ok(QuoteGate::To(KEY_NEW)) }
    let class = super::keys::venue(c, exchange_id(config))?.map(|venue| venue.class).unwrap_or_default();
    Ok(if refusal(c, app, &class)?.is_some() { QuoteGate::Refused } else { QuoteGate::Open })
}

/// GET .../pick_spendable_asset/new.
pub async fn quote_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, config) = (app.clone(), ctx.session.lock().bot_config.clone());
    match app.db(move |c| quote_gate(c, &inner, user.id, config.as_ref(), true)).await? {
        QuoteGate::To(path) => Ok(redirect(&ctx, path)),
        QuoteGate::Refused => Ok(layout::not_ported_for(&ctx)),
        QuoteGate::Open => quote_render(&app, &ctx, StatusCode::OK, vec![]).await,
    }
}

/// What saving the bot came to.
enum Saved { Created(i64), Invalid(String), Stale }

/// POST .../pick_spendable_asset: the quote joins the session's settings with the index wizard's defaults (quote_amount 100,
/// interval week, allocation_flattening 0.0, where none is set), and the bot is saved: created, not started. A blank quote
/// re-renders the step (422); a bot 2.x would not save re-renders it with the reason, the session keeping what was merged.
pub async fn quote_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let (inner, held) = (app.clone(), ctx.session.lock().bot_config.clone());
    match app.db(move |c| quote_gate(c, &inner, user.id, held.as_ref(), false)).await? {
        QuoteGate::To(path) => return Ok(redirect(&ctx, path)),
        QuoteGate::Refused => return Ok(layout::not_ported_for(&ctx)),
        QuoteGate::Open => {}
    }
    let Some(quote) = ctx.params.form(QUOTE_FIELD).or_else(|| ctx.params.query(QUOTE_FIELD)).map(str::to_string) else { return Ok(layout::bad_request()) };
    // #710 7a §2: a form rendered for another pick than the session's now (two tabs) makes nothing.
    let stale = || super::stream(StatusCode::CONFLICT, crate::web::turbo::redirect(&ctx.path(QUOTE_NEW)));
    if !super::still_bound(&ctx, ctx.session.lock().bot_config.as_ref()) { return Ok(stale()) }
    if crate::v2::blank(&quote) { return quote_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, vec![]).await }
    let config = {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        let settings = &mut config["settings"];
        if !settings.is_object() { *settings = json!({}); }
        settings["quote_asset_id"] = json!(search::to_i(&quote));
        // Bots::WizardDefaults::INDEX, `||=`.
        for (key, value) in [("quote_amount", json!(100)), ("interval", json!("week")), ("allocation_flattening", json!(0.0))] {
            if matches!(settings.get(key), None | Some(Value::Null | Value::Bool(false))) { settings[key] = value; }
        }
        config.clone()
    };
    let (inner, locale) = (app.clone(), ctx.locale);
    let saved = app.db(move |c| {
        let tx = model::immediate(c)?;
        let mut config = config;
        super::create::lock_limit_orders(&tx, &mut config)?;
        let bot = create::build(&tx, &config, locale)?;
        let errors = create::validate(&bot, locale);
        if !errors.is_empty() { return Ok(Saved::Invalid(crate::web::bot::draft::sentence(&errors.iter().map(String::as_str).collect::<Vec<_>>(), locale))) }
        let Ok(account) = super::bot_account(&tx, user.id, Some(&config))? else { return Ok(Saved::Stale) };
        let account = account.filter(|a| a.id.is_some());
        let id = create::insert(&tx, user.id, bot, account, inner.now())?;
        if let Err(refusal) = eligibility::guard(&tx, &inner.cipher, Some(id)) {
            return Ok(Saved::Invalid(i18n::text(locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&refusal.reason()))])));
        }
        tx.commit()?;
        inner.wake_engine();
        Ok(Saved::Created(id))
    }).await?;
    match saved {
        Saved::Created(id) => {
            ctx.session.lock().bot_config = None;
            Ok(super::stream(StatusCode::OK, crate::web::turbo::redirect(&ctx.path(&format!("/bots/{id}")))))
        }
        Saved::Invalid(message) => quote_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, vec![(flash::ALERT, message)]).await,
        Saved::Stale => Ok(stale()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn truncate_keeps_150_characters() {
        assert_eq!(super::truncate("short"), "short");
        let long = "é".repeat(151);
        assert_eq!(super::truncate(&long), format!("{}...", "é".repeat(147)));
        assert_eq!(super::truncate(&"a".repeat(150)), "a".repeat(150));
    }
}
