//! The index wizard (Bots::DcaIndexes::*): the CoinGecko step (SetupCoingeckosController, its form only: this build has no
//! CoinGecko provider, so the key it would save is a named refusal), the index picker (PickIndicesController), the venue
//! (PickExchangesController), the key (keys.rs, `Namespace::Index`) and the quote (PickSpendableAssetsController), whose POST
//! saves the bot (index_create). Every step but the first sends the browser to it while no market data is configured.
//!
//! What this engine runs is an index bot on Alpaca, Kraken or Binance on a data-api install (eligibility::admission and index_reasons);
//! the steps offer what Rails offers, and the key and quote steps refuse, by name, an index bot this build would not run.
use super::exchanges::{ordered, Table, Venue as Tile};
use super::index_create::{self as create, count_named, display_index_name, index_row};
use super::keys::FullPage;
use super::{dig, exchange_id, expired, progress_bar, step_complete, Step};
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

pub const SETUP_NEW: &str = "/bots/dca_indexes/setup_coingecko/new";
const SETUP: &str = "/bots/dca_indexes/setup_coingecko";
const PICK: &str = "/bots/dca_indexes/pick_index";
const PICK_NEW: &str = "/bots/dca_indexes/pick_index/new";
const EXCHANGE: &str = "/bots/dca_indexes/pick_exchange";
const EXCHANGE_NEW: &str = "/bots/dca_indexes/pick_exchange/new";
const KEY_NEW: &str = "/bots/dca_indexes/add_api_key/new";
const QUOTE: &str = "/bots/dca_indexes/pick_spendable_asset";
const QUOTE_NEW: &str = "/bots/dca_indexes/pick_spendable_asset/new";
const EXCHANGE_FIELD: &str = "bots_dca_index[exchange_id]";
const QUOTE_FIELD: &str = "bots_dca_index[quote_asset_id]";

/// Index::ExchangeAvailability::DISPLAY_COINS_COUNT and MINIMUM_SUPPORTED_COINS; the quote rows show six.
const TILE_PILLS: usize = 5;
const ROW_PILLS: usize = 6;
const MINIMUM_SUPPORTED_COINS: i64 = 3;

/// bots/dca_indexes/_new_step_progress_bar.
fn progress(ctx: &Ctx, step: &str) -> Result<String, WebError> {
    let share = match step { "coingecko" => 20, "index" => 40, "exchange" => 60, "api" => 80, _ => 100 };
    progress_bar(ctx.t(&format!("bot.setup.progress_steps.{step}")), share)
}

fn settings_of(config: Option<&Value>) -> Map<String, Value> {
    config.and_then(|config| config.get("settings")).and_then(Value::as_object).cloned().unwrap_or_default()
}

/// MarketData.configured?
fn configured(c: &Connection, app: &App) -> Result<bool, WebError> { Ok(crate::web::bots::market_data(c, app)?.1) }

/// Why this engine would not run an index bot built from the session on a venue of `class`, if it would not: a venue the
/// engine admits index bots on (eligibility::admission), on a data-api install.
pub fn refusal(c: &Connection, app: &App, _config: Option<&Value>, class: &str) -> Result<Option<&'static str>, WebError> {
    if let Some(why) = eligibility::admission(Some(class), true, false) { return Ok(Some(why)) }
    if !crate::web::bots::market_data(c, app)?.0 { return Ok(Some("an index bot whose market data comes from CoinGecko")) }
    Ok(None)
}

/// `session[:bot_config] ||= {}`.
fn begin(ctx: &Ctx) {
    let mut data = ctx.session.lock();
    if matches!(data.bot_config, None | Some(Value::Null) | Some(Value::Bool(false))) { data.bot_config = Some(json!({})); }
}

/// A step page in the wizard's dialog, its content made on the database thread.
async fn render<F>(app: &App, ctx: &Ctx, status: StatusCode, flash_now: Vec<(&'static str, String)>, content: F) -> Result<Response, WebError>
where F: FnOnce(&Connection, &Ctx, &str) -> Result<String, WebError> + Send + 'static {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("an index step without a user".into()))?;
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let (body, shell) = app.db(move |c| {
        let csrf = view.csrf_token();
        let content = content(c, &view, &csrf)?;
        Ok((FullPage { close_url: view.path("/bots"), content: &content }.render()?, Shell::load(c, &inner, &owner)?))
    }).await?;
    shell::application(ctx, &ctx.csrf_token(), &user, &shell, Page { status, body, flash_now })
}

fn redirect(ctx: &Ctx, path: &str) -> Response { layout::redirect(StatusCode::FOUND, &ctx.path(path)) }

#[derive(Template)]
#[template(path = "wizard/index/setup_coingecko.html")]
struct Setup<'a> { v: &'a Ctx, progress: String, admin: bool, title: String, how: String, action: String, csrf: &'a str }

/// GET .../setup_coingecko/new: market data configured, the index step (the wizard begun); else the CoinGecko key form for an
/// admin, and for anyone else who to ask.
pub async fn setup_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let inner = app.clone();
    if app.db(move |c| configured(c, &inner)).await? {
        begin(&ctx);
        return Ok(redirect(&ctx, PICK_NEW));
    }
    render(&app, &ctx, StatusCode::OK, vec![], move |_, ctx, csrf| {
        let title = i18n::t(ctx.locale, "bot.setup.connect_to", &[("name", i18n::Arg::Text("CoinGecko"))]);
        let how = i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", i18n::Arg::Text("CoinGecko"))]);
        Ok(Setup { v: ctx, progress: progress(ctx, "coingecko")?, admin: user.admin, title, how, action: ctx.path(SETUP), csrf }.render()?)
    }).await
}

struct Pill { id: i64, symbol: String, color: String }

struct IndexTile { index_type: &'static str, top: bool, name: String, external_id: String, description: String, summary: String, has_coins: bool, pills: Vec<Pill>, more: i64 }

#[derive(Template)]
#[template(path = "wizard/index/pick_index.html")]
struct Picker<'a> { v: &'a Ctx, progress: String, action: String, csrf: &'a str, tiles: Vec<IndexTile> }

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
/// then CoinGecko categories; by weight, then market cap.
fn tiles(c: &Connection, app: &App) -> Result<Vec<IndexTile>, WebError> {
    let deltabadger = crate::web::bots::market_data(c, app)?.0;
    let mut statement = c.prepare("SELECT external_id, source, name, description, top_coins, available_exchanges FROM indices WHERE ?1 OR source != 'deltabadger' \
                                   ORDER BY CASE source WHEN 'deltabadger' THEN 0 WHEN 'internal' THEN 1 ELSE 2 END, weight DESC, market_cap DESC, id")?;
    type IndexColumns = (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
    let rows = statement.query_map([deltabadger], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<Result<Vec<IndexColumns>, _>>()?;
    // `Asset.where(external_id: ...).index_by(&:external_id)`: the last row of an id wins.
    let wanted: Vec<String> = rows.iter().flat_map(|row| ids_of(row.4.clone())).collect();
    let mut assets: std::collections::HashMap<String, (i64, String, Option<String>)> = Default::default();
    let mut find = c.prepare("SELECT external_id, id, symbol, color FROM assets WHERE external_id IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
    for row in find.query_map([json!(wanted).to_string()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)))? {
        let (external, id, symbol, color) = row?;
        assets.insert(external, (id, symbol.unwrap_or_default(), color));
    }
    rows.into_iter().map(|(external_id, source, name, description, top_coins, available)| {
        let external_id = external_id.unwrap_or_default();
        let coins = ids_of(top_coins);
        let mut pills = vec![];
        for coin in &coins {
            if pills.len() >= TILE_PILLS { break }
            let Some((id, symbol, color)) = assets.get(coin) else { continue };
            pills.push(Pill { id: *id, symbol: symbol.clone(), color: colors::pill_color(color.as_deref()).ok_or_else(|| WebError::Config("an asset colour Ruby cannot read".into()))? });
        }
        let most = available.and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
            .values().filter_map(|count| count.as_i64().or_else(|| count.as_f64().map(|f| f as i64))).max().unwrap_or(0);
        let top = source.as_deref() == Some("internal");
        let description = description.unwrap_or_default();
        Ok(IndexTile {
            index_type: if top { "top" } else { "category" }, top,
            name: count_named(Some(&external_id)).map_or_else(|| name.unwrap_or_default(), |(_, named)| named.to_string()),
            summary: truncate(&description), description, has_coins: !coins.is_empty(), more: most - pills.len() as i64, pills, external_id,
        })
    }).collect()
}

async fn picker(app: &App, ctx: &Ctx, status: StatusCode, flash_now: Vec<(&'static str, String)>) -> Result<Response, WebError> {
    let inner = app.clone();
    render(app, ctx, status, flash_now, move |c, ctx, csrf| {
        Ok(Picker { v: ctx, progress: progress(ctx, "index")?, action: ctx.path(PICK), csrf, tiles: tiles(c, &inner)? }.render()?)
    }).await
}

/// GET .../pick_index/new.
pub async fn pick_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let inner = app.clone();
    if !app.db(move |c| configured(c, &inner)).await? { return Ok(redirect(&ctx, SETUP_NEW)) }
    begin(&ctx);
    picker(&app, &ctx, StatusCode::OK, vec![]).await
}

/// POST .../pick_index: the tile's index (Top Coins, or a category with its id) joins the session's settings; nothing chosen
/// re-renders the step (422) with the reason.
pub async fn pick_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let inner = app.clone();
    if !app.db(move |c| configured(c, &inner)).await? { return Ok(redirect(&ctx, SETUP_NEW)) }
    let param = |name: &str| ctx.params.form(name).or_else(|| ctx.params.query(name)).map(str::to_string);
    let (index_type, category, name) = (param("index_type"), param("index_category_id"), param("index_name"));
    let chosen = {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        if !config["settings"].is_object() { config["settings"] = json!({}); }
        let settings = config["settings"].as_object_mut().expect("an object, made one above");
        match (index_type.as_deref(), category.filter(|id| !crate::ruby::blank(id))) {
            (Some("top"), _) => {
                for key in ["index_type", "index_category_id", "index_name", "index_name_prefix"] { settings.insert(key.into(), Value::Null); }
                settings.insert("index_type".into(), json!("top"));
                true
            }
            (Some("category"), Some(id)) => {
                let prefix = count_named(Some(&id)).map(|(prefix, _)| prefix);
                settings.insert("index_type".into(), json!("category"));
                settings.insert("index_category_id".into(), json!(id));
                settings.insert("index_name".into(), json!(name));
                settings.insert("index_name_prefix".into(), json!(prefix));
                true
            }
            _ => false,
        }
    };
    if chosen { return Ok(redirect(&ctx, EXCHANGE_NEW)) }
    picker(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, vec![(flash::ALERT, ctx.t("bot.dca_index.setup.pick_index.error"))]).await
}

/// The before_actions of the venue step: market data, then an index chosen.
fn exchange_gate(c: &Connection, app: &App, config: Option<&Value>) -> Result<Option<&'static str>, WebError> {
    if !configured(c, app)? { return Ok(Some(SETUP_NEW)) }
    let chosen = dig(config, &["settings", "index_type"]).is_some_and(|v| v.as_str().is_some_and(|t| !crate::ruby::blank(t)) || matches!(v, Value::Number(_) | Value::Bool(true)));
    Ok((!chosen).then_some(PICK_NEW))
}

/// The sentence of the venue and quote steps: "Invest in {index} portfolio at {venue} spending {quote}".
struct SentenceVenue { name_id: String, mark: &'static str, name: String }

#[derive(Template)]
#[template(path = "wizard/index/_sentence.html")]
struct Sentence<'a> { v: &'a Ctx, index_url: String, index_name: String, venue: Option<SentenceVenue>, exchange_url: String, search_url: String }

/// The index's name as the bot built from the session shows it, after_initialize's coin count included.
fn index_name(c: &Connection, config: Option<&Value>, locale: &str) -> Result<String, WebError> {
    let mut settings = settings_of(config);
    let bounded = create::bounded(index_row(c, &settings)?.as_ref());
    if matches!(settings.get("num_coins"), None | Some(Value::Null)) { settings.insert("num_coins".into(), json!(bounded.map_or(10, |size| size.min(10)))); }
    Ok(display_index_name(&settings, bounded, locale))
}

fn sentence(c: &Connection, ctx: &Ctx, config: Option<&Value>, venue: Option<&super::keys::Venue>, search: &str) -> Result<String, WebError> {
    let venue = venue.map(|venue| {
        let name_id = crate::tracker::rows::name_id(&venue.class);
        SentenceVenue { mark: crate::web::bot::exchange_svg(&name_id), name_id, name: venue.name.clone() }
    });
    Ok(Sentence { v: ctx, index_url: ctx.path(PICK_NEW), index_name: index_name(c, config, ctx.locale)?, venue, exchange_url: ctx.path(EXCHANGE_NEW),
                  search_url: ctx.path(search) }.render()?)
}

#[derive(Template)]
#[template(path = "wizard/index/pick_exchange.html")]
struct ExchangeStep { progress: String, sentence: String, table: Table }

/// `exchange_search_results_for_index_bot`: the venues a category index is available on (every available venue when it names
/// none, or for Top Coins), ordered or searched as Bots::Searchable does.
fn exchange_page(c: &Connection, ctx: &Ctx, csrf: &str, config: Option<&Value>) -> Result<String, WebError> {
    let settings = settings_of(config);
    let category = settings.get("index_category_id").and_then(Value::as_str).filter(|id| !crate::ruby::blank(id));
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
        id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), class: r.get::<_, Option<String>>(2)?.unwrap_or_default(), maker_fee: None,
    }))?.collect::<Result<Vec<_>, _>>()?;
    let venues = ordered(venues, ctx.params.query("query")).into_iter()
        .map(|venue| Tile { id: venue.id, mark: crate::web::bot::exchange_wordmark(&venue.name_id()), unavailable: false }).collect();
    let table = Table { action: ctx.path(EXCHANGE), csrf: csrf.to_string(), param_name: EXCHANGE_FIELD.into(), venues, unavailable_note: ctx.t("bot.hyperliquid_trading_unavailable") };
    Ok(ExchangeStep { progress: progress(ctx, "exchange")?, sentence: sentence(c, ctx, config, None, EXCHANGE_NEW)?, table }.render()?)
}

async fn exchange_render(app: &App, ctx: &Ctx, status: StatusCode) -> Result<Response, WebError> {
    let config = ctx.session.lock().bot_config.clone();
    render(app, ctx, status, vec![], move |c, ctx, csrf| exchange_page(c, ctx, csrf, config.as_ref())).await
}

/// GET .../pick_exchange/new.
pub async fn exchange_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let (inner, config) = (app.clone(), ctx.session.lock().bot_config.clone());
    if let Some(path) = app.db(move |c| exchange_gate(c, &inner, config.as_ref())).await? { return Ok(redirect(&ctx, path)) }
    begin(&ctx);
    exchange_render(&app, &ctx, StatusCode::OK).await
}

/// POST .../pick_exchange: the venue joins the session as the form sent it; a blank one re-renders the step (422).
pub async fn exchange_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let (inner, config) = (app.clone(), ctx.session.lock().bot_config.clone());
    if let Some(path) = app.db(move |c| exchange_gate(c, &inner, config.as_ref())).await? { return Ok(redirect(&ctx, path)) }
    let Some(exchange) = ctx.params.form(EXCHANGE_FIELD).or_else(|| ctx.params.query(EXCHANGE_FIELD)).map(str::to_string) else { return Ok(layout::bad_request()) };
    if crate::ruby::blank(&exchange) { return exchange_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY).await }
    {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        config["exchange_id"] = json!(exchange);
    }
    Ok(redirect(&ctx, KEY_NEW))
}

#[derive(Template)]
#[template(path = "wizard/index/_api_key_form.html")]
struct KeyForm<'a> { v: &'a Ctx, progress: String, name_id: String, mark: &'static str, name: &'a str, exchange_url: String, card: &'a str }

/// bots/dca_indexes/add_api_keys/_form: "Build index portfolio at {venue}" over the venue's connect card.
pub fn key_form(ctx: &Ctx, name: &str, class: &str, card: &str) -> Result<String, WebError> {
    let name_id = crate::tracker::rows::name_id(class);
    Ok(KeyForm { v: ctx, progress: progress(ctx, "api")?, mark: crate::web::bot::exchange_svg(&name_id), name_id, name, exchange_url: ctx.path(EXCHANGE_NEW), card }.render()?)
}

struct QuoteRow { item: Item, pills: Vec<Pill>, more: i64 }

#[derive(Template)]
#[template(path = "wizard/index/pick_spendable_asset.html")]
struct QuoteStep { progress: String, sentence: String, create_url: String, csrf: String, rows: Vec<QuoteRow> }

/// The quote step: Bots::DcaIndex#available_assets_for_current_settings (the quotes in which the venue lists at least three of
/// the index's members; Top Coins any three) ranked or searched, one page of them, each with the index's members it pairs
/// with on the venue (top_base_assets_by_quote_for_current_setup), by market-cap rank.
fn quote_page(c: &Connection, ctx: &Ctx, csrf: &str, config: Option<&Value>) -> Result<String, WebError> {
    let settings = settings_of(config);
    let venue = super::keys::venue(c, exchange_id(config))?;
    let index = index_row(c, &settings)?;
    let category = settings.get("index_type").and_then(Value::as_str) == Some("category")
        && settings.get("index_category_id").and_then(Value::as_str).is_some_and(|id| !crate::ruby::blank(id));
    let members = |venue: &super::keys::Venue| index.as_ref().map(|index| index.top_coins_for_exchange(&venue.class)).unwrap_or_default();
    // `base_asset_ids if coin_ids.present?`, narrowing only when any is an Asset.
    let narrowed: Option<String> = match (&venue, category) {
        (Some(venue), true) => {
            let coins = members(venue);
            let ids: Vec<i64> = if coins.is_empty() { vec![] } else {
                let mut s = c.prepare("SELECT id FROM assets WHERE external_id IN (SELECT value FROM json_each(?1))")?;
                let ids = s.query_map([json!(coins).to_string()], |r| r.get(0))?.collect::<Result<_, _>>()?;
                ids
            };
            (!ids.is_empty()).then(|| json!(ids).to_string())
        }
        _ => None,
    };
    let available = format!("id IN (SELECT quote_asset_id FROM tickers WHERE available = 1 AND trading_enabled = 1 \
                             AND exchange_id IN (SELECT id FROM exchanges WHERE (?1 IS NULL AND available = 1) OR id = ?1) \
                             AND (?2 IS NULL OR base_asset_id IN (SELECT value FROM json_each(?2))) GROUP BY quote_asset_id HAVING COUNT(*) >= {MINIMUM_SUPPORTED_COINS})");
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
                by_quote.push((quote, Pill { id, symbol: symbol.unwrap_or_default(), color: colors::pill_color(color.as_deref()).ok_or_else(|| WebError::Config("an asset colour Ruby cannot read".into()))? }));
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
    Ok(QuoteStep { progress: progress(ctx, "spendable")?, sentence: sentence(c, ctx, config, venue.as_ref(), QUOTE_NEW)?, create_url: ctx.path(QUOTE), csrf: csrf.to_string(), rows }.render()?)
}

async fn quote_render(app: &App, ctx: &Ctx, status: StatusCode, flash_now: Vec<(&'static str, String)>) -> Result<Response, WebError> {
    let config = ctx.session.lock().bot_config.clone();
    render(app, ctx, status, flash_now, move |c, ctx, csrf| quote_page(c, ctx, csrf, config.as_ref())).await
}

/// What the quote step does before its page: no market data, the CoinGecko step; the key not correct, the key step; an index
/// bot this engine would not run, refused.
enum QuoteGate { To(&'static str), Refused, Open }

fn quote_gate(c: &Connection, app: &App, user: i64, config: Option<&Value>, check_key: bool) -> Result<QuoteGate, WebError> {
    if !configured(c, app)? { return Ok(QuoteGate::To(SETUP_NEW)) }
    if check_key && !step_complete(c, user, config, Step::Api)? { return Ok(QuoteGate::To(KEY_NEW)) }
    let class = super::keys::venue(c, exchange_id(config))?.map(|venue| venue.class).unwrap_or_default();
    Ok(if refusal(c, app, config, &class)?.is_some() { QuoteGate::Refused } else { QuoteGate::Open })
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
enum Saved { Created(i64), Invalid(String) }

/// POST .../pick_spendable_asset: the quote joins the session's settings with the index wizard's defaults (quote_amount 100,
/// interval week, allocation_flattening 0.0, where none is set), and the bot is saved: created, not started. A blank quote
/// re-renders the step (422); a bot Rails would not save re-renders it with the reason, the session keeping what was merged.
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
    if crate::ruby::blank(&quote) { return quote_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, vec![]).await }
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
        let bot = create::build(&tx, &config, locale)?;
        let errors = create::validate(&bot, locale);
        if !errors.is_empty() { return Ok(Saved::Invalid(crate::web::bot::draft::sentence(&errors.iter().map(String::as_str).collect::<Vec<_>>(), locale))) }
        let id = create::insert(&tx, user.id, bot, inner.now())?;
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
