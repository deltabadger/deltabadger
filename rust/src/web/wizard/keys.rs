//! The DCA wizard's API-key step (Bots::Wizard::AddApiKeysController), in the single namespace (exchange-first, before any
//! asset) and the multi one (a basket's venue). `new` moves on when the user's trading key for the chosen venue is correct,
//! and asks the venue again, on the GET, when a stored key is not known to be (the one wizard GET that calls a venue and
//! writes); `create` checks the submitted key and saves it when it is correct. Alpaca, Kraken, Binance and KuCoin are checked here;
//! any other venue's key step is a named refusal, so the wizard never makes a bot this build does not trade. The index wizard's
//! key step (Bots::DcaIndexes::AddApiKeysController) is the third namespace, for an index this engine runs (index::refusal).
use super::{basket_ids, chips, current_variant, exchange_id, exchange_name, first_incomplete, prerequisite, progress, sentence, step_complete, BotType,
            SentenceOf, Step, StepOrder};
use crate::web::layout::{self, Ctx, Page};
use crate::web::settings::{key_check, keys};
use crate::web::shell::{self, Shell};
use crate::web::{auth, flash, i18n, turbo, App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Namespace { Single, Multi, Index }

impl Namespace {
    fn bot_type(self) -> BotType { if self == Namespace::Single { BotType::Single } else { BotType::Multi } }
    fn action(self) -> &'static str {
        match self { Namespace::Single => "/bots/dca_single_assets/add_api_key", Namespace::Multi => "/bots/dca_multi_assets/add_api_key", Namespace::Index => "/bots/dca_indexes/add_api_key" }
    }
}

/// The venues whose keys this build checks.
pub const PORTED: [&str; 4] = ["Exchanges::Alpaca", "Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Kucoin"];

/// `bot.exchange`: the row the session's exchange id names.
#[derive(Clone)]
pub struct Venue { pub id: i64, pub name: String, pub class: String }

pub fn venue(c: &Connection, id: Option<i64>) -> Result<Option<Venue>, WebError> {
    let Some(id) = id else { return Ok(None) };
    Ok(c.query_row("SELECT id, name, type FROM exchanges WHERE id = ?1", [id], |r| Ok(Venue { id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                                                                                                class: r.get::<_, Option<String>>(2)?.unwrap_or_default() })).optional()?)
}

/// WizardSteps#stock_bot?: a chosen asset is a stock.
pub fn stock_bot(c: &Connection, config: Option<&Value>) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE category = 'Stock' AND id IN (SELECT value FROM json_each(?1)))",
                   [serde_json::json!(basket_ids(config)).to_string()], |r| r.get(0))?)
}

/// `missing_exchange_path`: the single step's exchange picker, or the basket's (its broker picker for stocks).
fn missing_exchange_path(c: &Connection, config: Option<&Value>, namespace: Namespace) -> Result<&'static str, WebError> {
    Ok(match namespace {
        Namespace::Single => "/bots/dca_single_assets/pick_exchange/new",
        Namespace::Multi if stock_bot(c, config)? => "/bots/dca_multi_assets/pick_stock_broker/new",
        Namespace::Multi => "/bots/dca_multi_assets/pick_exchange/new",
        Namespace::Index => "/bots/dca_indexes/pick_exchange/new",
    })
}

/// Each namespace's `step_path`.
pub(crate) fn step_path(c: &Connection, config: Option<&Value>, namespace: Namespace, step: Step) -> Result<&'static str, WebError> {
    Ok(match step {
        Step::Currencies | Step::Assets => "/bots/dca_single_assets/pick_buyable_asset/new",
        Step::Exchange => missing_exchange_path(c, config, namespace)?,
        Step::Api if namespace == Namespace::Single => "/bots/dca_single_assets/add_api_key/new",
        Step::Api => "/bots/dca_multi_assets/add_api_key/new",
        Step::Spendable => "/bots/dca_multi_assets/pick_spendable_asset/new",
    })
}

fn order(config: Option<&Value>, namespace: Namespace) -> StepOrder { StepOrder::new(namespace.bot_type(), current_variant(config)) }

/// `after_api_key_path`: the first step still missing input, the key now in the database.
fn after_api_key_path(c: &Connection, user: i64, config: Option<&Value>, namespace: Namespace) -> Result<&'static str, WebError> {
    if namespace == Namespace::Index { return Ok("/bots/dca_indexes/pick_spendable_asset/new") }
    step_path(c, config, namespace, first_incomplete(c, user, config, &order(config, namespace))?)
}

/// `prerequisite_redirect_path`. The single step comes before any asset: once a basket is chosen its key step is the basket's.
fn prerequisite_path(c: &Connection, user: i64, config: Option<&Value>, namespace: Namespace) -> Result<Option<&'static str>, WebError> {
    if namespace == Namespace::Index { return Ok(None) }
    if namespace == Namespace::Single && step_complete(c, user, config, Step::Assets)? { return Ok(Some("/bots/dca_multi_assets/add_api_key/new")) }
    prerequisite(c, user, config, &order(config, namespace), Step::Api)?.map(|step| step_path(c, config, namespace, step)).transpose()
}

#[derive(Template)]
#[template(path = "wizard/full_page.html")]
pub struct FullPage<'a> { pub close_url: String, pub content: &'a str }

#[derive(Template)]
#[template(path = "wizard/_api_key_form.html")]
struct Form { progress: String, sentence: String, card: String }

/// The step's `_form`: progress, sentence and the venue's connect card. `live`: the Paper/Live switch on Live (Alpaca).
#[allow(clippy::too_many_arguments)]
fn form(c: &Connection, app: &App, ctx: &Ctx, csrf: &str, config: Option<&Value>, namespace: Namespace, venue: &Venue, live: bool) -> Result<String, WebError> {
    let name_id = crate::tracker::rows::name_id(&venue.class);
    let proxy = crate::venue::http::proxy_for(&name_id, crate::app_config::get(c, &app.cipher, &format!("proxy_{name_id}")).map_err(WebError::Config)?, &|k| std::env::var(k).ok());
    let instructions = key_check::instructions_html(ctx.locale, "bot.api", &name_id, &venue.name, &key_check::whitelist_ip_html(ctx.locale, proxy.as_deref())).unwrap_or_default();
    let card = keys::card(ctx, csrf, ctx.path(namespace.action()), "data-controller=\"form--label-animations form--html5-validations\" data-turbo-frame=\"modal_content\"",
                          &venue.name, &venue.class, live, false, instructions)?;
    if namespace == Namespace::Index { return super::index::key_form(ctx, &venue.name, &venue.class, &card) }
    let order = order(config, namespace);
    let multi = namespace == Namespace::Multi;
    let ids = if multi { basket_ids(config) } else { vec![] };
    let sentence = sentence(ctx, csrf, SentenceOf { order: &order, step: Step::Api, multi, chips: chips(c, &ids)?, exchange: exchange_name(c, Some(venue.id))? })?;
    Ok(Form { progress: progress(ctx, &order, Step::Api)?, sentence, card }.render()?)
}

/// What the step decided before any venue is asked.
enum Route { To(&'static str), Retired(&'static str), Refused, Ready(Venue, Box<keys::Key>) }

/// The checks `new` and `create` share, in Rails' order: a retired venue, (on `new`) an earlier step not complete, no venue chosen.
fn route(c: &Connection, app: &App, user: i64, config: Option<&Value>, namespace: Namespace, on_new: bool) -> Result<Route, WebError> {
    // The index step's before_action: no market data, no index wizard.
    if namespace == Namespace::Index && !crate::web::bots::market_data(c, app)?.1 { return Ok(Route::To(super::index::SETUP_NEW)) }
    let venue = venue(c, exchange_id(config))?;
    if venue.as_ref().is_some_and(|venue| crate::web::bot::retired_type(&venue.class)) { return Ok(Route::Retired(missing_exchange_path(c, config, namespace)?)) }
    if on_new { if let Some(path) = prerequisite_path(c, user, config, namespace)? { return Ok(Route::To(path)) } }
    if exchange_id(config).is_none() { return Ok(Route::To(missing_exchange_path(c, config, namespace)?)) }
    let Some(venue) = venue.filter(|venue| PORTED.contains(&venue.class.as_str())) else { return Ok(Route::Refused) };
    if namespace == Namespace::Index && super::index::refusal(c, app, config, &venue.class)?.is_some() { return Ok(Route::Refused) }
    let key = keys::Key::read(c, app, user, venue.id, 0)?;
    Ok(Route::Ready(venue, Box::new(key)))
}

/// RetiredExchangeGuard#reject_retired_exchange: the reason, on the next page.
fn retired(ctx: &Ctx, path: &str) -> Response {
    flash::set(&ctx.session, flash::ALERT, ctx.t("errors.exchange_retired"));
    layout::redirect(StatusCode::FOUND, &ctx.path(path))
}

async fn new(app: App, ctx: Ctx, namespace: Namespace) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.clone();
    let (inner, held) = (app.clone(), config.clone());
    let routed = app.db(move |c| route(c, &inner, user.id, held.as_ref(), namespace, true)).await?;
    let (venue, key) = match routed {
        Route::To(path) => return Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path))),
        Route::Retired(path) => return Ok(retired(&ctx, path)),
        Route::Refused => return Ok(layout::not_ported_for(&ctx)),
        Route::Ready(venue, key) => (venue, key),
    };
    // Only a key that is there and not known to be correct is asked about again.
    let present = |value: &Option<String>| value.as_deref().is_some_and(|text| !crate::ruby::blank(text));
    let mut correct = key.status == 1;
    if let (false, true, true, Some(id)) = (correct, present(&key.key), present(&key.secret), key.id) {
        if let Some(reason) = keys::revalidate(&app, id).await? {
            let body = turbo::prepend_flash(&flash::render(&flash::take(&ctx.session, &[(flash::ALERT, i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&reason))]))]))?);
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response());
        }
        correct = app.db(move |c| Ok(c.query_row("SELECT status FROM api_keys WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).optional()? == Some(1))).await?;
    }
    let (inner, view, live, owner) = (app.clone(), ctx.clone(), key.passphrase.as_deref() == Some("live"), user.clone());
    let page = app.db(move |c| {
        if correct { return Ok(Err(after_api_key_path(c, owner.id, config.as_ref(), namespace)?)) }
        let csrf = view.csrf_token();
        let content = form(c, &inner, &view, &csrf, config.as_ref(), namespace, &venue, live)?;
        Ok(Ok((FullPage { close_url: view.path("/bots"), content: &content }.render()?, Shell::load(c, &inner, &owner)?)))
    }).await?;
    match page {
        Err(path) => Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path))),
        Ok((body, shell)) => shell::application(&ctx, &ctx.csrf_token(), &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] }),
    }
}

/// POST: the submitted key, checked on its venue. Correct: saved, and the browser goes on (a turbo-stream redirect out of the
/// form's frame). Otherwise nothing is saved: the form again with the reason (422), the key's own permissions named where the
/// venue reports them.
async fn create(app: App, ctx: Ctx, namespace: Namespace) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.clone();
    let (inner, held) = (app.clone(), config.clone());
    let (venue, key) = match app.db(move |c| route(c, &inner, user.id, held.as_ref(), namespace, false)).await? {
        Route::To(path) => return Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path))),
        Route::Retired(path) => return Ok(retired(&ctx, path)),
        Route::Refused => return Ok(layout::not_ported_for(&ctx)),
        Route::Ready(venue, key) => (venue, key),
    };
    // `params.require(:api_key)`.
    if !ctx.params.form.iter().any(|(name, _)| name.starts_with("api_key[")) { return Ok(layout::bad_request()) }
    let live = ctx.params.form("api_key[passphrase]").map_or(key.passphrase.as_deref() == Some("live"), |mode| mode == "live");
    let message = match keys::submit(&app, &ctx, user.id, venue.id, &venue.class, &venue.name, 0, *key).await? {
        keys::Submitted::Correct => {
            let path = app.db(move |c| after_api_key_path(c, user.id, config.as_ref(), namespace)).await?;
            return Ok(super::stream(StatusCode::OK, turbo::redirect(&ctx.path(path))));
        }
        keys::Submitted::Incorrect => i18n::text(ctx.locale, "errors.incorrect_api_key_permissions", &[]),
        keys::Submitted::Permissions(missing, forbidden) => {
            key_check::permission_message(ctx.locale, &crate::tracker::rows::name_id(&venue.class), &venue.name, 0, &missing, &forbidden)
        }
        keys::Submitted::Failed => i18n::text(ctx.locale, "errors.api_key_permission_validation_failed", &[]),
        keys::Submitted::Refused(reason) => i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&reason))]),
    };
    let (inner, view) = (app.clone(), ctx.clone());
    let content = app.db(move |c| { let csrf = view.csrf_token(); form(c, &inner, &view, &csrf, config.as_ref(), namespace, &venue, live) }).await?;
    // create.turbo_stream: the form in place of the step's frame, then the reason (flash.now).
    let flash = flash::render(&flash::take(&ctx.session, &[(flash::ALERT, message)]))?;
    let body = format!("{}\n{}\n", turbo::stream("update", "modal_content", &format!("{content}\n")), turbo::prepend_flash(flash.trim_end()));
    Ok(super::stream(StatusCode::UNPROCESSABLE_ENTITY, body))
}

pub async fn single_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { new(app, ctx, Namespace::Single).await }
pub async fn single_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { create(app, ctx, Namespace::Single).await }
pub async fn multi_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { new(app, ctx, Namespace::Multi).await }
pub async fn multi_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { create(app, ctx, Namespace::Multi).await }
pub async fn index_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { new(app, ctx, Namespace::Index).await }
pub async fn index_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { create(app, ctx, Namespace::Index).await }
