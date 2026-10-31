//! The signal wizard (Bots::Signals::*, plan 8e): the asset to trade (PickBuyableAssetsController), the venue
//! (PickExchangesController), the key (keys.rs, `Namespace::Signal`), the quote (PickSpendableAssetsController), the rules
//! (ConfirmSettingsController, kept in `session[:bot_config]['signals']` until the bot exists) and the save (SignalsController:
//! the bot and its rules, then started). Each step builds the bot from the session as Rails does; the asset and quote lists are
//! the shared search (bot::search) with a one-pair scope.
//!
//! Venues: the exchange step offers only the venues this engine runs signal bots on (eligibility::signal_admission). Rails'
//! own way of showing a venue it cannot trade on (bots/pick_exchanges/_table's disabled button) names Hyperliquid's missing
//! adapter, so it does not fit; a refused venue is left out instead, and one sent anyway is refused by name at its key step.
use super::exchanges::{ordered, Table, Venue as Tile};
use super::keys::{self, FullPage};
use super::{chips, dig, exchange_id, expired, progress_bar, step_complete, Chip, Step};
use crate::engine::{eligibility, model};
use crate::ruby::BigDec;
use crate::web::bot::search::{self, AssetPage, Basket, Item, Side};
use crate::web::bot::signal_write::{self, Started};
use crate::web::layout::{self, Ctx, Page};
use crate::web::shell::{self, Shell};
use crate::web::{auth, i18n, turbo, App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

const ASSETS: &str = "/bots/signals/pick_buyable_asset";
const ASSETS_NEW: &str = "/bots/signals/pick_buyable_asset/new";
const EXCHANGE: &str = "/bots/signals/pick_exchange";
const EXCHANGE_NEW: &str = "/bots/signals/pick_exchange/new";
const KEY_NEW: &str = "/bots/signals/add_api_key/new";
const QUOTE: &str = "/bots/signals/pick_spendable_asset";
const QUOTE_NEW: &str = "/bots/signals/pick_spendable_asset/new";
const CONFIRM: &str = "/bots/signals/confirm_settings";
const CONFIRM_NEW: &str = "/bots/signals/confirm_settings/new";
const BASE_FIELD: &str = "bots_signal[base_asset_id]";
const EXCHANGE_FIELD: &str = "bots_signal[exchange_id]";
const QUOTE_FIELD: &str = "bots_signal[quote_asset_id]";

/// bots/signals/_new_step_progress_bar.
fn progress(ctx: &Ctx, step: &str) -> Result<String, WebError> {
    let share = match step { "currencies" => 20, "exchange" => 40, "spendable" => 60, "api" => 80, _ => 100 };
    progress_bar(ctx.t(&format!("bot.setup.progress_steps.{step}")), share)
}

/// `bot.base_asset_id` / `quote_asset_id` of the bot built from the session: the stored id, nil when blank.
fn setting_id(config: Option<&Value>, key: &str) -> Option<i64> {
    config?.get("settings")?.get(key).filter(|v| match v { Value::Null => false, Value::String(t) => !crate::ruby::blank(t), _ => true }).map(super::to_i)
}

fn base_id(config: Option<&Value>) -> Option<i64> { setting_id(config, "base_asset_id") }
fn quote_id(config: Option<&Value>) -> Option<i64> { setting_id(config, "quote_asset_id") }

/// `params.require(:bots_signal)[field]`: None when the field was not sent at all (ParameterMissing).
fn field(ctx: &Ctx, name: &str) -> Option<String> { ctx.params.form(name).or_else(|| ctx.params.query(name)).map(str::to_string) }

/// `session[:bot_config] ||= {}`, an object to write into.
fn config_mut(config: &mut Option<Value>) -> &mut serde_json::Map<String, Value> {
    if !config.as_ref().is_some_and(Value::is_object) { *config = Some(json!({})); }
    config.as_mut().and_then(Value::as_object_mut).expect("an object, made one above")
}

/// `session[:bot_config].deep_merge!(settings: { key => id })`.
fn merge_setting(ctx: &Ctx, key: &str, id: i64) {
    let mut data = ctx.session.lock();
    let config = config_mut(&mut data.bot_config);
    let settings = config.entry("settings").or_insert_with(|| json!({}));
    if !settings.is_object() { *settings = json!({}); }
    settings[key] = json!(id);
}

fn missing(what: &str) -> WebError { WebError::Config(format!("a signal wizard step with no {what} in the session")) }

/// The base Asset as its pill shows it (`bot.base_asset`, which every later step's sentence reads).
fn base_chip(c: &Connection, config: Option<&Value>) -> Result<Chip, WebError> {
    let id = base_id(config).ok_or_else(|| missing("asset"))?;
    chips(c, &[id])?.into_iter().next().ok_or_else(|| missing("asset row"))
}

struct SentenceVenue { name_id: String, mark: &'static str, name: String }

fn sentence_venue(venue: &keys::Venue) -> SentenceVenue {
    let name_id = crate::tracker::rows::name_id(&venue.class);
    SentenceVenue { mark: crate::web::bot::exchange_svg(&name_id), name_id, name: venue.name.clone() }
}

#[derive(Template)]
#[template(path = "wizard/signals/_sentence.html")]
struct Sentence<'a> { v: &'a Ctx, base: Option<Chip>, assets_url: String, venue: Option<SentenceVenue>, exchange_url: String, search: Option<(&'static str, String)> }

/// "Invest in {asset} at {venue} spending {search}": the base from the exchange step on, the venue from the key step on.
fn sentence(c: &Connection, ctx: &Ctx, config: Option<&Value>, base: bool, venue: bool, search: Option<(&'static str, &str)>) -> Result<String, WebError> {
    let base = if base { Some(base_chip(c, config)?) } else { None };
    let venue = if venue { Some(sentence_venue(&keys::venue(c, exchange_id(config))?.ok_or_else(|| missing("exchange"))?)) } else { None };
    Ok(Sentence { v: ctx, base, assets_url: ctx.path(ASSETS_NEW), venue, exchange_url: ctx.path(EXCHANGE_NEW), search: search.map(|(frame, url)| (frame, ctx.path(url))) }.render()?)
}

#[derive(Template)]
#[template(path = "wizard/signals/_api_key_form.html")]
struct KeyForm { progress: String, sentence: String, card: String }

/// bots/signals/add_api_keys/_form: "Invest in {asset} at {venue}" over the venue's connect card.
pub fn key_form(c: &Connection, ctx: &Ctx, config: Option<&Value>, card: &str) -> Result<String, WebError> {
    // The partial opens with the line that assigns the pill's colour: a line break before its markup.
    Ok(format!("\n{}", KeyForm { progress: progress(ctx, "api")?, sentence: sentence(c, ctx, config, true, true, None)?, card: card.to_string() }.render()?))
}

#[derive(Template)]
#[template(path = "wizard/signals/pick_asset.html")]
struct PickAsset<'a> {
    progress: String,
    sentence: String,
    create_url: String,
    csrf: &'a str,
    field_name: &'static str,
    items: Vec<Item>,
    next: Option<(String, String)>,
    base_step: bool,
}

/// One side of the asset steps: the base (any venue, the quote if one is chosen) or the quote (the chosen venue and base).
struct AssetStep { side: Side, step: &'static str, field: &'static str, create: &'static str, search: &'static str }

const BASE_STEP: AssetStep = AssetStep { side: Side::Base, step: "currencies", field: BASE_FIELD, create: ASSETS, search: ASSETS_NEW };
const QUOTE_STEP: AssetStep = AssetStep { side: Side::Quote, step: "spendable", field: QUOTE_FIELD, create: QUOTE, search: QUOTE_NEW };

/// `prepare_step` and the page (Bots::Searchable#asset_search_results over Bot::AssetConfigurable#offered_tickers, the step's
/// own asset cleared), or (`render_asset_page`, a lazy frame asking for `offset`) one page of results.
fn asset_page(c: &Connection, ctx: &Ctx, config: Option<&Value>, at: &AssetStep, partial_allowed: bool) -> Result<(String, bool), WebError> {
    let scope = match at.side {
        Side::Base => Basket { exchange_id: None, quote_asset_id: quote_id(config), base_asset_ids: vec![] },
        Side::Quote => Basket { exchange_id: exchange_id(config), quote_asset_id: None, base_asset_ids: base_id(config).into_iter().collect() },
    };
    let params = &ctx.params;
    let offset = params.query("offset").map_or(0, search::to_i);
    let all = search::filtered_rows(c, &scope, at.side, params.query("query"))?;
    let next_offset = offset.saturating_add(search::PAGE_SIZE as i64);
    let next = (all.len() as i64 > next_offset).then(|| {
        let query = params.fullpath.split_once('?').map_or("", |(_, query)| query);
        (format!("assets-page-{next_offset}"), format!("{}?{}", params.full_path, search::next_page_query(query, next_offset)))
    });
    let mut rows = search::page_of(all, offset);
    search::attach_exchanges(c, &scope, at.side, &mut rows)?;
    let items = search::items(rows)?;
    if partial_allowed && offset > 0 {
        return Ok((AssetPage { frame_id: format!("assets-page-{offset}"), field_name: at.field.into(), items, next }.render()?, true));
    }
    let csrf = ctx.csrf_token();
    let quote = at.side == Side::Quote;
    let content = PickAsset {
        progress: progress(ctx, at.step)?, sentence: sentence(c, ctx, config, quote, quote, Some(("asset-picker", at.search)))?, create_url: ctx.path(at.create),
        csrf: &csrf, field_name: at.field, items, next, base_step: !quote,
    }.render()?;
    Ok((FullPage { close_url: ctx.path("/bots"), content: &content }.render()?, false))
}

/// A step page through the layout, its content made on the database thread; a lazy page of results bare.
async fn render<F>(app: &App, ctx: &Ctx, status: StatusCode, content: F) -> Result<Response, WebError>
where F: FnOnce(&Connection, &Ctx) -> Result<(String, bool), WebError> + Send + 'static {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("a signal wizard step without a user".into()))?;
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let ((body, partial), shell) = app.db(move |c| Ok((content(c, &view)?, Shell::load(c, &inner, &owner)?))).await?;
    if partial { return Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()) }
    shell::application(ctx, &ctx.csrf_token(), &user, &shell, Page { status, body, flash_now: vec![] })
}

async fn assets_render(app: &App, ctx: &Ctx, status: StatusCode, at: &'static AssetStep, partial_allowed: bool) -> Result<Response, WebError> {
    let config = ctx.session.lock().bot_config.clone();
    render(app, ctx, status, move |c, ctx| asset_page(c, ctx, config.as_ref(), at, partial_allowed)).await
}

fn redirect(ctx: &Ctx, path: &str) -> Response { layout::redirect(StatusCode::FOUND, &ctx.path(path)) }

/// GET .../pick_buyable_asset/new: the wizard's first step (`session[:bot_config] ||= {}`).
pub async fn assets_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    config_mut(&mut ctx.session.lock().bot_config);
    assets_render(&app, &ctx, StatusCode::OK, &BASE_STEP, true).await
}

/// POST .../pick_buyable_asset: the asset joins the session's settings (a first pick needs no session) and the venue step
/// follows; a blank one re-renders the step (422).
pub async fn assets_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let Some(id) = field(&ctx, BASE_FIELD) else { return Ok(layout::bad_request()) };
    if crate::ruby::blank(&id) { return assets_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, &BASE_STEP, false).await }
    merge_setting(&ctx, "base_asset_id", search::to_i(&id));
    Ok(redirect(&ctx, EXCHANGE_NEW))
}

#[derive(Template)]
#[template(path = "wizard/index/pick_exchange.html")]
struct ExchangeStep { progress: String, sentence: String, table: Table }

/// `prepare_step` and the page: the venues listing the pair (Bot::AssetConfigurable#available_exchanges_for_current_settings, the
/// venue cleared) that this engine runs signal bots on, ordered or searched as Bots::Searchable does.
fn exchange_page(c: &Connection, ctx: &Ctx, config: Option<&Value>) -> Result<(String, bool), WebError> {
    let scope = Basket { exchange_id: None, quote_asset_id: quote_id(config), base_asset_ids: vec![] };
    let venues = search::available_exchanges(c, &scope, Some(base_id(config)))?.into_iter()
        .filter(|venue| eligibility::signal_admission(Some(&venue.class)).is_none()).collect();
    let venues = ordered(venues, ctx.params.query("query")).into_iter()
        .map(|venue| Tile { id: venue.id, mark: crate::web::bot::exchange_wordmark(&venue.name_id()), unavailable: false }).collect();
    let table = Table { action: ctx.path(EXCHANGE), csrf: ctx.csrf_token(), param_name: EXCHANGE_FIELD.into(), venues, unavailable_note: ctx.t("bot.hyperliquid_trading_unavailable") };
    let content = ExchangeStep { progress: progress(ctx, "exchange")?, sentence: sentence(c, ctx, config, true, false, Some(("exchange-picker", EXCHANGE_NEW)))?, table }.render()?;
    Ok((FullPage { close_url: ctx.path("/bots"), content: &content }.render()?, false))
}

async fn exchange_render(app: &App, ctx: &Ctx, status: StatusCode) -> Result<Response, WebError> {
    let config = ctx.session.lock().bot_config.clone();
    render(app, ctx, status, move |c, ctx| exchange_page(c, ctx, config.as_ref())).await
}

/// GET .../pick_exchange/new: no asset yet, the asset step.
pub async fn exchange_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if base_id(ctx.session.lock().bot_config.as_ref()).is_none() { return Ok(redirect(&ctx, ASSETS_NEW)) }
    exchange_render(&app, &ctx, StatusCode::OK).await
}

/// POST .../pick_exchange: the venue joins the session as the form sent it, and the key step follows; a blank one re-renders
/// the step (422).
pub async fn exchange_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let Some(exchange) = field(&ctx, EXCHANGE_FIELD) else { return Ok(layout::bad_request()) };
    if crate::ruby::blank(&exchange) { return exchange_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY).await }
    config_mut(&mut ctx.session.lock().bot_config).insert("exchange_id".into(), json!(exchange));
    Ok(redirect(&ctx, KEY_NEW))
}

/// GET .../pick_spendable_asset/new: the key for the venue not correct, the key step.
pub async fn quote_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.clone();
    if !app.db(move |c| Ok(step_complete(c, user.id, config.as_ref(), Step::Api)?)).await? { return Ok(redirect(&ctx, KEY_NEW)) }
    assets_render(&app, &ctx, StatusCode::OK, &QUOTE_STEP, true).await
}

/// POST .../pick_spendable_asset: the quote joins the session's settings and the rules follow; a blank one re-renders the
/// step (422).
pub async fn quote_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let Some(id) = field(&ctx, QUOTE_FIELD) else { return Ok(layout::bad_request()) };
    if crate::ruby::blank(&id) { return assets_render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, &QUOTE_STEP, false).await }
    merge_setting(&ctx, "quote_asset_id", search::to_i(&id));
    Ok(redirect(&ctx, CONFIRM_NEW))
}

// ---------------------------------------------------------------------------------------------------------------------
// The rules: Bots::Signals::ConfirmSettingsController, on `session[:bot_config]['signals']`.

/// A rule of the session as `bot.bot_signals.build(direction:, amount:, enabled:, amount_type:)` makes it.
struct Rule { sell: bool, percentage: bool, amount: Option<BigDec>, enabled: bool }

impl Rule {
    /// BotSignal's validations: amount present and above 0, and at most 100 for a percentage.
    fn valid(&self) -> bool { self.amount.as_ref().is_some_and(|a| a.is_positive() && (!self.percentage || *a <= BigDec::from_i64(100))) }
}

/// The default rule: buy for 100 of the quote.
fn default_rule() -> Value { json!({"direction": "buy", "amount": 100, "enabled": true}) }

/// The session's rules, built. An enum value Rails does not know raises (ArgumentError) as it does there.
fn rules(signals: &[Value]) -> Result<Vec<Rule>, WebError> {
    signals.iter().map(|rule| {
        let name = |key: &str, default: &str, names: [&str; 2]| -> Result<bool, WebError> {
            match rule.get(key).map_or(Some(default), Value::as_str) {
                Some(text) if text == names[0] => Ok(false),
                Some(text) if text == names[1] => Ok(true),
                _ => Err(WebError::Config(format!("a wizard rule's {key} is not one Rails knows"))),
            }
        };
        let sell = name("direction", "", ["buy", "sell"])?;
        let percentage = name("amount_type", "fixed", ["fixed", "percentage"])?;
        // ActiveModel::Type::Decimal: an Integer as it is, a Float by Float#to_d, a text read as a number (else nil).
        let amount = match rule.get("amount") {
            Some(Value::Number(n)) => match n.as_i64() { Some(i) => Some(BigDec::from_i64(i)), None => n.as_f64().map(BigDec::from_f64).transpose().map_err(|e| WebError::Config(format!("{e:?}")))? },
            Some(Value::String(text)) => BigDec::parse(text.trim()).ok(),
            _ => None,
        };
        // `wh.fetch('enabled', true)`, cast as a boolean column casts it; nil is not enabled.
        let enabled = rule.get("enabled").map_or(Some(true), signal_write::boolean).unwrap_or(false);
        Ok(Rule { sell, percentage, amount, enabled })
    }).collect()
}

/// `signal.amount.to_d.to_s('F').sub(/([0-9]\d*)\.0$/, '\1')`.
fn amount_text(amount: &BigDec) -> String {
    let text = amount.to_s_f();
    text.strip_suffix(".0").map_or_else(|| text.clone(), str::to_string)
}

struct Pill { class: &'static str, pill: String, symbol: Option<String>, id: Option<i64> }

struct RuleView { sell: bool, percentage: bool, enabled: bool, amount: Option<String>, update_url: String, remove_url: String }

#[derive(Template)]
#[template(path = "wizard/signals/_settings.html")]
struct Settings<'a> { v: &'a Ctx, csrf: &'a str, base: Pill, quote: Pill, quote_symbol: String, rules: Vec<RuleView>, several: bool, add_url: String }

/// bots/signals/_settings of the bot not saved yet: its pair and the rules of the session.
fn settings(c: &Connection, ctx: &Ctx, csrf: &str, config: Option<&Value>, rules: &[Rule]) -> Result<String, WebError> {
    let asset = |id: Option<i64>| -> Result<Option<crate::web::bot::Asset>, WebError> { id.map(|id| crate::web::bot::Asset::find(c, id)).transpose().map(Option::flatten) };
    let (base, quote) = (asset(base_id(config))?, asset(quote_id(config))?);
    let pill = |asset: &Option<crate::web::bot::Asset>| -> Result<Pill, WebError> {
        let color = asset.as_ref().and_then(|a| a.color.as_deref());
        Ok(Pill { class: crate::web::colors::ticker_class(asset.as_ref().and_then(|a| a.category.as_deref()), color),
                  pill: crate::web::colors::pill_color(color).ok_or_else(|| WebError::Config("an asset colour Ruby cannot read".into()))?,
                  symbol: asset.as_ref().and_then(|a| a.symbol.clone()), id: asset.as_ref().map(|a| a.id) })
    };
    // `bot.quote_asset.symbol`: each rule reads it.
    let quote_symbol = match (&quote, rules.is_empty()) { (Some(q), _) => q.symbol().to_string(), (None, true) => String::new(), (None, false) => return Err(missing("quote")) };
    let views = rules.iter().enumerate().map(|(index, rule)| RuleView {
        sell: rule.sell, percentage: rule.percentage, enabled: rule.enabled, amount: rule.amount.as_ref().map(amount_text),
        update_url: ctx.path(&format!("{CONFIRM}/update_signal?signal_index={index}")), remove_url: ctx.path(&format!("{CONFIRM}/remove_signal?signal_index={index}")),
    }).collect();
    let quote_pill = Pill { class: "ticker", pill: String::new(), symbol: quote.as_ref().and_then(|a| a.symbol.clone()), id: quote.as_ref().map(|a| a.id) };
    Ok(Settings { v: ctx, csrf, base: pill(&base)?, quote: quote_pill, quote_symbol, rules: views,
                  several: rules.len() > 1, add_url: ctx.path(&format!("{CONFIRM}/add_signal")) }.render()?)
}

#[derive(Template)]
#[template(path = "wizard/signals/_create_bot_button.html")]
struct Button<'a> { action: String, csrf: &'a str, start: String, starting: String, disabled: bool }

/// bots/signals/_create_bot_button. Its form posts to `bots_signals_path(bot)`, where the unsaved bot is the locale: no prefix.
fn button(ctx: &Ctx, csrf: &str, rules: &[Rule]) -> Result<String, WebError> {
    Ok(Button { action: "/bots/signals".into(), csrf, start: ctx.t("button.start"), starting: ctx.t("busy.starting"), disabled: !rules.iter().all(Rule::valid) }.render()?)
}

#[derive(Template)]
#[template(path = "wizard/signals/confirm_settings.html")]
struct Confirm { progress: String, name_id: String, mark: &'static str, name: String, exchange_url: String, button: String, settings: String }

/// The session's rules (`session[:bot_config]['signals']`): a wizard not begun, or not yet at this step, is Rails' NoMethodError.
fn session_rules(config: Option<&Value>) -> Result<Vec<Value>, WebError> {
    dig(config, &["signals"]).and_then(Value::as_array).cloned().ok_or_else(|| missing("rules"))
}

/// GET .../confirm_settings/new: the rules begin as one (`||=`), and the bot as it would start.
pub async fn confirm_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let config = {
        let mut data = ctx.session.lock();
        let Some(config) = data.bot_config.as_mut().and_then(Value::as_object_mut) else { return Err(missing("wizard")) };
        if matches!(config.get("signals"), None | Some(Value::Null | Value::Bool(false))) { config.insert("signals".into(), json!([default_rule()])); }
        data.bot_config.clone()
    };
    render(&app, &ctx, StatusCode::OK, move |c, ctx| {
        let rules = rules(&session_rules(config.as_ref())?)?;
        let venue = sentence_venue(&keys::venue(c, exchange_id(config.as_ref()))?.ok_or_else(|| missing("exchange"))?);
        let csrf = ctx.csrf_token();
        let content = Confirm { progress: progress(ctx, "confirm")?, name_id: venue.name_id, mark: venue.mark, name: venue.name, exchange_url: ctx.path(EXCHANGE_NEW),
                                button: button(ctx, &csrf, &rules)?, settings: settings(c, ctx, &csrf, config.as_ref(), &rules)? }.render()?;
        Ok((FullPage { close_url: ctx.path("/bots"), content: &content }.render()?, false))
    }).await
}

fn stream_response(body: String) -> Response { super::stream(StatusCode::OK, body) }

/// The rules' column again (`turbo_stream.replace('new-settings', ...)`).
async fn settings_stream(app: &App, ctx: &Ctx) -> Result<Response, WebError> {
    let (view, config) = (ctx.clone(), ctx.session.lock().bot_config.clone());
    let body = app.db(move |c| {
        let rules = rules(&session_rules(config.as_ref())?)?;
        Ok(turbo::stream("replace", "new-settings", &format!("{}\n", settings(c, &view, &view.csrf_token(), config.as_ref(), &rules)?)))
    }).await?;
    Ok(stream_response(body))
}

/// POST .../confirm_settings: the bot built from the session without its rules, which is valid (no validation of a new
/// signal bot reads anything the wizard leaves unset): its column (no rules) and its Start button again.
pub async fn confirm_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let (view, config) = (ctx.clone(), ctx.session.lock().bot_config.clone());
    let body = app.db(move |c| {
        let csrf = view.csrf_token();
        Ok([turbo::stream("replace", "new-settings", &format!("{}\n", settings(c, &view, &csrf, config.as_ref(), &[])?)),
            turbo::stream("replace", "create-bot-button", &format!("{}\n", button(&view, &csrf, &[])?)),
            turbo::prepend_flash("")].join("\n") + "\n")
    }).await?;
    Ok(stream_response(body))
}

/// `signal_index.to_i`, read from the query or the form.
fn index(ctx: &Ctx) -> i64 { field(ctx, "signal_index").map_or(0, |text| search::to_i(&text)) }

/// Ruby's Array index: a negative one counts from the end.
fn at(len: usize, index: i64) -> Option<usize> {
    let at = if index < 0 { index + len as i64 } else { index };
    (0..len as i64).contains(&at).then_some(at as usize)
}

/// The session's rules, to change in place.
fn with_rules<T>(ctx: &Ctx, change: impl FnOnce(&mut Vec<Value>) -> T) -> Result<T, WebError> {
    let mut data = ctx.session.lock();
    let rules = data.bot_config.as_mut().and_then(|c| c.get_mut("signals")).and_then(Value::as_array_mut).ok_or_else(|| missing("rules"))?;
    Ok(change(rules))
}

/// POST .../confirm_settings/add_signal: one more default rule.
pub async fn add_signal(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    with_rules(&ctx, |rules| rules.push(default_rule()))?;
    settings_stream(&app, &ctx).await
}

/// DELETE .../confirm_settings/remove_signal: the rule at `signal_index`, unless it is the last one.
pub async fn remove_signal(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let index = index(&ctx);
    with_rules(&ctx, |rules| if rules.len() > 1 { if let Some(at) = at(rules.len(), index) { rules.remove(at); } })?;
    settings_stream(&app, &ctx).await
}

/// `head`: no body, and the Turbo Stream format's type without a charset.
fn head(status: StatusCode) -> Response { (status, [(header::CONTENT_TYPE, "text/vnd.turbo-stream.html")]).into_response() }

/// PATCH .../confirm_settings/update_signal: what the rule's form sent (each field only when present; the amount by
/// String#to_f). The column comes back only when the toggle was sent (more than one rule), so typing is not interrupted.
pub async fn update_signal(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    let index = index(&ctx);
    let present = |name: &str| field(&ctx, name).filter(|text| !crate::ruby::blank(text));
    let (direction, amount, amount_type, enabled) = (present("direction"), present("amount"), present("amount_type"), field(&ctx, "enabled"));
    let amount = amount.map(|text| crate::web::bot::action_params::numeric(&json!(text), false).map_err(|_| WebError::Config("an amount past what Ruby reads".into()))).transpose()?;
    let found = with_rules(&ctx, |rules| {
        let Some(rule) = at(rules.len(), index).and_then(|at| rules[at].as_object_mut()) else { return false };
        if let Some(direction) = direction { rule.insert("direction".into(), json!(direction)); }
        if let Some(amount) = amount { rule.insert("amount".into(), amount); }
        if let Some(amount_type) = amount_type { rule.insert("amount_type".into(), json!(amount_type)); }
        if let Some(enabled) = &enabled { rule.insert("enabled".into(), json!(enabled == "1")); }
        true
    })?;
    if !found { return Ok(head(StatusCode::NOT_FOUND)) }
    if enabled.is_none() { return Ok(head(StatusCode::OK)) }
    settings_stream(&app, &ctx).await
}

// ---------------------------------------------------------------------------------------------------------------------
// The save: Bots::SignalsController#create (Bots::Wizard::CreatesController).

/// What the save came to.
enum Saved { Started(i64, Vec<(String, String)>), Refused(String, bool), Unported, Unservable(&'static str) }

/// POST /bots/signals: the bot built from the session with its rules (the default one when the session has none) is saved, each
/// rule with a token of its own, and started. A rule Rails would not save refuses the whole save (422, nothing written); a start
/// the model refuses leaves the saved bot as it is (422, the reasons), as Rails does; a write the engine's guard refuses leaves
/// nothing. A session without a venue or a pair, or one whose bot no page here could serve, is refused by name, nothing written.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.clone();
    let signals = dig(config.as_ref(), &["signals"]).filter(|v| !v.is_null()).and_then(Value::as_array).cloned().unwrap_or_else(|| vec![json!({"direction": "buy", "amount": 100})]);
    let built = rules(&signals)?;
    if !built.iter().all(Rule::valid) {
        // The rules are validated with the bot (validate_associated_records_for_bot_signals): one message for the attribute.
        let body = signal_write::alert(signal_write::model_message(ctx.locale, "bot_signals", "invalid"))?;
        return Ok(super::stream(StatusCode::UNPROCESSABLE_ENTITY, body));
    }
    let mut ctx = ctx;
    ctx.now = app.now();
    let view = ctx.clone();
    let saved = app.db(move |c| {
        let config = config.as_ref();
        let (Some(exchange), Some(base), Some(_)) = (exchange_id(config), base_id(config), quote_id(config)) else { return Ok(Saved::Unported) };
        let tx = model::immediate(c)?;
        let now = crate::codec::format_time(view.now);
        // Automation::Labelable: the base's name, or "New bot".
        let name: Option<String> = tx.query_row("SELECT name FROM assets WHERE id = ?1", [base], |r| r.get(0)).optional()?.flatten();
        let label = name.filter(|n| !crate::ruby::blank(n)).unwrap_or_else(|| i18n::text(view.locale, "bot.new", &[]));
        // Bot#assign_position: after the user's last bot.
        let position: i64 = tx.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM bots WHERE user_id = ?1", [user.id], |r| r.get(0))?;
        let settings = dig(config, &["settings"]).cloned().unwrap_or_else(|| json!({}));
        tx.execute("INSERT INTO bots (user_id, exchange_id, type, status, label, position, settings, transient_data, settings_changed_at, created_at, updated_at) \
                    VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, '{}', ?7, ?7, ?7)",
                   rusqlite::params![user.id, exchange, model::SIGNAL_TYPE, label, position, settings.to_string(), now])?;
        let id = tx.last_insert_rowid();
        for rule in &built {
            let amount = rule.amount.as_ref().ok_or_else(|| WebError::Config("a validated rule without its amount".into()))?.to_s_f();
            tx.execute("INSERT INTO bot_signals (bot_id, direction, amount_type, amount, enabled, token, last_triggered_at, created_at, updated_at) \
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?7)",
                       rusqlite::params![id, rule.sell as i64, rule.percentage as i64, amount, rule.enabled, signal_write::new_token(&tx)?, now])?;
        }
        let started = signal_write::start_created(&tx, &view, user.id, id)?;
        // A bot whose pages this build cannot serve (`bot::refusal`, as a rule write checks it: an enabled sell rule while the
        // wash-sale question is unanswered, say) is not made at all. Rails saves it and asks the question on its page; here it
        // would be a bot no page, list or action could reach, so nothing is written and the reason is named.
        if !matches!(started, Started::Refused(_)) {
            if let Some(reason) = signal_write::refused_after(&tx, &view.app, id)? { return Ok(Saved::Unservable(reason)) }
        }
        match started {
            Started::Yes(broadcasts) => {
                tx.commit()?;
                Ok(Saved::Started(id, broadcasts))
            }
            Started::Invalid(message) => {
                // The saved bot stays (Rails saved it before the start), as long as the engine would hold it.
                if let Err(refusal) = eligibility::guard(&tx, &view.app.cipher, Some(id)) {
                    return Ok(Saved::Refused(i18n::text(view.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&refusal.reason()))]), false));
                }
                tx.commit()?;
                Ok(Saved::Refused(message, true))
            }
            Started::Refused(message) => Ok(Saved::Refused(message, false)),
        }
    }).await?;
    match saved {
        Saved::Started(id, broadcasts) => {
            ctx.session.lock().bot_config = None;
            app.wake_engine();
            for (stream, html) in broadcasts { app.hub.broadcast(&stream, &html); }
            Ok(super::stream(StatusCode::OK, turbo::redirect(&ctx.path(&format!("/bots/{id}")))))
        }
        Saved::Refused(message, written) => {
            // A saved bot the engine has not seen yet.
            if written { app.wake_engine(); }
            Ok(super::stream(StatusCode::UNPROCESSABLE_ENTITY, signal_write::alert(message)?))
        }
        Saved::Unported => Ok(layout::not_ported_for(&ctx)),
        Saved::Unservable(reason) => Ok(layout::refused(&ctx, reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_indexes_and_amount_text() {
        assert_eq!((at(3, 0), at(3, 2), at(3, 3), at(3, -1), at(3, -4)), (Some(0), Some(2), None, Some(2), None));
        assert_eq!(amount_text(&BigDec::from_f64(40.0).unwrap()), "40");
        assert_eq!(amount_text(&BigDec::from_f64(12.5).unwrap()), "12.5");
        assert_eq!(amount_text(&BigDec::from_i64(100)), "100");
    }

    #[test]
    fn rules_read_the_session_as_rails_builds_them() {
        let built = rules(&[json!({"direction": "sell", "amount": 150.0, "amount_type": "percentage"}), json!({"direction": "buy", "amount": 0, "enabled": false})]).unwrap();
        assert!(built[0].sell && built[0].percentage && built[0].enabled && !built[0].valid(), "over 100 %");
        assert!(!built[1].sell && !built[1].enabled && !built[1].valid(), "not above 0");
        assert!(rules(&[json!({"direction": "hold", "amount": 1})]).is_err(), "an enum Rails does not know raises");
        assert!(rules(&[default_rule()]).unwrap()[0].valid());
    }

    /// A venue the engine runs no signal bot on is never offered (the exchange step's filter).
    #[test]
    fn only_admitted_venues_are_offered() {
        for (class, offered) in [("Exchanges::Kraken", true), ("Exchanges::Alpaca", true), ("Exchanges::Ibkr", false), ("Exchanges::Coinbase", false)] {
            assert_eq!(eligibility::signal_admission(Some(class)).is_none(), offered, "{class}");
        }
    }
}
