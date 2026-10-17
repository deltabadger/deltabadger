//! The DCA wizard's stock-broker step (Bots::DcaMultiAssets::PickStockBrokersController): a stock basket picked asset-first
//! that more than one stock venue lists comes here instead of the crypto exchange picker. A broker is committed with the USD
//! quote default (Bots::StockBrokerRoutable#finalize_stock_broker!). Interactive Brokers is offered as Rails offers it; its key
//! step is a venue this build does not trade (wizard::keys), the refusal the plan names.
use super::keys::{stock_bot, FullPage};
use super::{basket_ids, current_variant, dig, progress, BotType, Step, StepOrder, QUOTE_KEY};
use crate::web::bot::exchange_wordmark;
use crate::web::bot::search::Basket;
use crate::web::layout::{self, Ctx, Page};
use crate::web::shell::{self, Shell};
use crate::web::wizard::exchanges::{Table, Venue};
use crate::web::{auth, App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::Connection;
use serde_json::{json, Value};

const REPICK: &str = "/bots/dca_single_assets/pick_buyable_asset/new";
const FIELD: &str = "bots_dca_multi_asset[exchange_id]";

/// `available_stock_brokers(build_bot)`: the stock venues listing every chosen asset, quoted in USD unless a quote is chosen.
fn brokers(c: &Connection, config: Option<&Value>) -> Result<Vec<(i64, String)>, WebError> {
    let quote = dig(config, QUOTE_KEY).filter(|value| !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())).map(super::to_i);
    let basket = Basket { exchange_id: super::exchange_id(config), quote_asset_id: quote, base_asset_ids: basket_ids(config) };
    let ids = super::assets::stock_brokers(c, &basket, super::usd_asset(c)?)?;
    let mut statement = c.prepare("SELECT type FROM exchanges WHERE id = ?1")?;
    ids.into_iter().map(|id| Ok((id, statement.query_row([id], |r| r.get::<_, String>(0))?))).collect()
}

#[derive(Template)]
#[template(path = "wizard/pick_stock_broker.html")]
struct New { progress: String, table: Table }

fn page(ctx: &Ctx, config: Option<&Value>, brokers: Vec<(i64, String)>) -> Result<String, WebError> {
    let order = StepOrder::new(BotType::Multi, current_variant(config));
    let venues = brokers.into_iter().map(|(id, class)| Venue { id, mark: exchange_wordmark(&crate::tracker::rows::name_id(&class)), unavailable: false }).collect();
    let table = Table { action: ctx.path("/bots/dca_multi_assets/pick_stock_broker"), csrf: ctx.csrf_token(), param_name: FIELD.into(), venues,
                        unavailable_note: ctx.t("bot.hyperliquid_trading_unavailable") };
    let content = New { progress: progress(ctx, &order, Step::Exchange)?, table }.render()?;
    Ok(FullPage { close_url: ctx.path("/bots"), content: &content }.render()?)
}

/// What the step answers, decided with the session as it stands.
enum Answer { Repick, Page(String, Shell), Chosen(i64, Option<i64>) }

async fn answer(app: &App, ctx: &Ctx, chosen: Option<String>) -> Result<Answer, WebError> {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("the broker step without a user".into()))?;
    let config = ctx.session.lock().bot_config.clone();
    let (inner, view) = (app.clone(), ctx.clone());
    app.db(move |c| {
        if !stock_bot(c, config.as_ref())? { return Ok(Answer::Repick) }
        let brokers = brokers(c, config.as_ref())?;
        if chosen.is_none() && brokers.is_empty() { return Ok(Answer::Repick) }
        if let Some(id) = chosen.as_deref().and_then(|wanted| brokers.iter().find(|(id, _)| id.to_string() == wanted).map(|(id, _)| *id)) {
            return Ok(Answer::Chosen(id, super::usd_asset(c)?));
        }
        Ok(Answer::Page(page(&view, config.as_ref(), brokers)?, Shell::load(c, &inner, &user)?))
    }).await
}

/// GET .../pick_stock_broker/new: the brokers; a basket with no stock, or one no broker lists, back to the assets.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    match answer(&app, &ctx, None).await? {
        Answer::Page(body, shell) => shell::application(&ctx, &ctx.csrf_token(), &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] }),
        _ => Ok(layout::redirect(StatusCode::FOUND, &ctx.path(REPICK))),
    }
}

/// POST .../pick_stock_broker: a broker of the list is committed with the USD quote and the browser goes to its key step; any
/// other id re-renders the step (422).
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    // `params.require(:bots_dca_multi_asset)`.
    let Some(chosen) = ctx.params.form(FIELD).or_else(|| ctx.params.query(FIELD)).map(str::to_string) else { return Ok(layout::bad_request()) };
    match answer(&app, &ctx, Some(chosen)).await? {
        Answer::Repick => Ok(layout::redirect(StatusCode::FOUND, &ctx.path(REPICK))),
        Answer::Page(body, shell) => shell::application(&ctx, &ctx.csrf_token(), &user, &shell, Page { status: StatusCode::UNPROCESSABLE_ENTITY, body, flash_now: vec![] }),
        Answer::Chosen(broker, usd) => {
            finalize(ctx.session.lock().bot_config.get_or_insert_with(|| json!({})), broker, usd);
            Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/bots/dca_multi_assets/add_api_key/new")))
        }
    }
}

/// Bots::StockBrokerRoutable#finalize_stock_broker!: the broker's id, and the USD quote deep-merged into the settings.
pub fn finalize(config: &mut Value, broker: i64, usd: Option<i64>) {
    if !config.is_object() { *config = json!({}); }
    config["exchange_id"] = json!(broker);
    if let Some(usd) = usd {
        let settings = &mut config["settings"];
        if !settings.is_object() { *settings = json!({}); }
        settings["quote_asset_id"] = json!(usd);
    }
}
