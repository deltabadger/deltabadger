//! The DCA wizard's last step (Bots::DcaMultiAssets::PickSpendableAssetsController): the quote the basket is bought with, then
//! the bot is saved in its created state (wizard::create) and the browser leaves the modal for the bot's page. Every GET here
//! leaves the session alone; the POST writes the quote and WizardDefaults into it, and clears it once the bot exists.
use super::keys::{step_path, Namespace};
use super::{basket_ids, chips, current_variant, exchange_id, exchange_name, expired, prerequisite, steer, step_complete, BotType, SentenceOf, Step,
            StepOrder};
use crate::engine::{eligibility, model};
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

const FIELD: &str = "bots_dca_multi_asset[quote_asset_id]";

#[derive(Template)]
#[template(path = "wizard/pick_spendable_asset.html")]
struct New { search: String, create_url: String, csrf: String, field_name: &'static str, items: Vec<Item>, next: Option<(String, String)> }

/// An asset step's pane (D-SHEET-EVERYWHERE): `search` over the results (frame `asset-picker`), each posting `field_name` to
/// `create_url`, and the lazy frame of the next page.
pub fn asset_pane(search: String, create_url: String, csrf: String, field_name: &'static str, items: Vec<Item>, next: Option<(String, String)>) -> Result<String, WebError> {
    Ok(New { search, create_url, csrf, field_name, items, next }.render()?)
}

/// `prepare_step` and the page: the quotes the venue lists every chosen asset in (the bot built with its quote cleared), or
/// (a lazy frame asking for `offset`) one page of them.
fn page(c: &Connection, ctx: &Ctx, config: Option<&Value>, partial_allowed: bool) -> Result<(String, bool), WebError> {
    let ids = basket_ids(config);
    let basket = Basket { exchange_id: exchange_id(config), quote_asset_id: None, base_asset_ids: ids.clone() };
    let params = &ctx.params;
    let offset = params.query("offset").map_or(0, search::to_i);
    let all = search::filtered_rows(c, &basket, Side::Quote, params.query("query"))?;
    let next_offset = offset.saturating_add(search::PAGE_SIZE as i64);
    let next = (all.len() as i64 > next_offset).then(|| {
        let query = params.fullpath.split_once('?').map_or("", |(_, query)| query);
        (format!("assets-page-{next_offset}"), format!("{}?{}", params.full_path, search::next_page_query(query, next_offset)))
    });
    let mut rows = search::page_of(all, offset);
    search::attach_exchanges(c, &basket, Side::Quote, &mut rows)?;
    let items = search::items(rows)?;
    if partial_allowed && offset > 0 {
        return Ok((AssetPage { frame_id: format!("assets-page-{offset}"), field_name: FIELD.into(), items, next }.render()?, true));
    }
    let csrf = ctx.csrf_token();
    let order = StepOrder::new(BotType::Multi, current_variant(config));
    let steered = steer(ctx, &csrf, SentenceOf { order: &order, step: Step::Spendable, multi: true, chips: chips(c, &ids)?, exchange: exchange_name(c, exchange_id(config))? }, true)?;
    let content = New { search: steered.search, create_url: ctx.path("/bots/dca_multi_assets/pick_spendable_asset"), csrf,
                        field_name: FIELD, items, next }.render()?;
    Ok((super::sheet(ctx, &steered.track, &content)?, false))
}

async fn render(app: &App, ctx: &Ctx, status: StatusCode, partial_allowed: bool, flash_now: Vec<(&'static str, String)>) -> Result<Response, WebError> {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("the quote step without a user".into()))?;
    let config = ctx.session.lock().bot_config.clone();
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let ((body, partial), shell) = app.db(move |c| Ok((page(c, &view, config.as_ref(), partial_allowed)?, Shell::load(c, &inner, &owner)?))).await?;
    if partial { return Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()) }
    shell::application(ctx, &ctx.csrf_token(), &user, &shell, Page { status, body, flash_now })
}

/// GET .../pick_spendable_asset/new: an earlier step that is not complete (the key among them: it lives in the database)
/// sends the browser back to it.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.clone();
    let back = app.db(move |c| {
        let config = config.as_ref();
        let order = StepOrder::new(BotType::Multi, current_variant(config));
        if let Some(step) = prerequisite(c, user.id, config, &order, Step::Spendable)? { return Ok(Some(step_path(c, config, Namespace::Multi, step)?)) }
        Ok((!step_complete(c, user.id, config, Step::Api)?).then_some("/bots/dca_multi_assets/add_api_key/new"))
    }).await?;
    if let Some(path) = back { return Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path))) }
    render(&app, &ctx, StatusCode::OK, true, vec![]).await
}

/// What saving the bot came to.
enum Saved { Created(i64), Invalid(String) }

/// POST .../pick_spendable_asset: the quote joins the session's settings with the wizard's defaults (quote_amount 100, interval
/// week, where none is set), and the bot is saved: created, not started. A blank quote re-renders the step (422); a bot 2.x
/// would not save re-renders it with the reason, the session keeping what was merged into it.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    if let Some(answer) = expired(&ctx) { return Ok(answer) }
    let Some(quote) = ctx.params.form(FIELD).or_else(|| ctx.params.query(FIELD)).map(str::to_string) else { return Ok(layout::bad_request()) };
    if crate::v2::blank(&quote) { return render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, false, vec![]).await }
    let config = {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        let settings = &mut config["settings"];
        if !settings.is_object() { *settings = json!({}); }
        // `bot.parse_params(bot_params)`: the quote's `to_i`, deep-merged; then Bots::WizardDefaults.apply! (`||=`).
        settings["quote_asset_id"] = json!(crate::web::bot::search::to_i(&quote));
        for (key, value) in [("quote_amount", json!(100)), ("interval", json!("week"))] {
            if matches!(settings.get(key), None | Some(Value::Null | Value::Bool(false))) { settings[key] = value; }
        }
        config.clone()
    };
    let (inner, locale) = (app.clone(), ctx.locale);
    let saved = app.db(move |c| {
        let tx = model::immediate(c)?;
        let mut config = config;
        super::create::lock_limit_orders(&tx, &mut config)?;
        let bot = super::create::build(&tx, &config, locale)?;
        // The picked account (#710 7a §2), else the exchange's number 1.
        let account = super::account(&tx, user.id, Some(&config))?;
        let errors = super::create::validate(&tx, &bot, account.as_ref(), locale)?;
        if !errors.is_empty() { return Ok(Saved::Invalid(crate::web::bot::draft::sentence(&errors.iter().map(String::as_str).collect::<Vec<_>>(), locale))) }
        let id = super::create::insert_as(&tx, user.id, bot, inner.now(), crate::enums::BotStatus::Created, 0, account.filter(|a| a.id.is_some()))?;
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
        Saved::Invalid(message) => render(&app, &ctx, StatusCode::UNPROCESSABLE_ENTITY, false, vec![(flash::ALERT, message)]).await,
    }
}
