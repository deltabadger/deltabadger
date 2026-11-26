//! The DCA wizard's stock-broker step (Bots::DcaMultiAssets::PickStockBrokersController): a stock basket picked asset-first
//! that more than one stock venue lists comes here instead of the crypto exchange picker. A broker is committed with the USD
//! quote default (Bots::StockBrokerRoutable#finalize_stock_broker!). Interactive Brokers is offered as 2.x offers it; its key
//! step is a venue this build does not trade (wizard::keys), the refusal the plan names.
use super::keys::stock_bot;
use super::{basket_ids, current_variant, dig, BotType, Step, StepOrder, QUOTE_KEY};
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

/// `available_stock_brokers(build_bot)`: the stock venues listing every chosen asset, quoted in USD unless a quote is chosen; for
/// a mixed selection only those `grouped::eligible` admits (PLAN index-as-ingredient slice 6), on the GET and the POST alike.
fn brokers(c: &Connection, app: &App, config: Option<&Value>) -> Result<Vec<(i64, String)>, WebError> {
    let quote = dig(config, QUOTE_KEY).filter(|value| !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())).map(super::to_i);
    let basket = Basket { exchange_id: super::exchange_id(config), quote_asset_id: quote, base_asset_ids: basket_ids(config) };
    let mut ids = super::assets::stock_brokers(c, &basket, super::usd_asset(c)?)?;
    if super::grouped::mixed(config) {
        let mut admitted = vec![];
        for id in ids { if super::grouped::session_eligible(c, app, config, id, quote)? { admitted.push(id); } }
        ids = admitted;
    }
    let mut statement = c.prepare("SELECT type FROM exchanges WHERE id = ?1")?;
    ids.into_iter().map(|id| Ok((id, statement.query_row([id], |r| r.get::<_, String>(0))?))).collect()
}

#[derive(Template)]
#[template(path = "wizard/pick_stock_broker.html")]
struct New { table: Table }

/// #710 7a §2: the broker's tiles, one per usable account (a correct trading key) when it has two; one plain tile otherwise,
/// carrying its account only when that one is numbered (2+ live accounts), so a single-account broker keeps 2.x's tile.
pub fn tiles(c: &Connection, user: i64, broker: i64) -> rusqlite::Result<Vec<(Option<i64>, Option<i64>)>> {
    let usable: Vec<_> = super::exchanges::connected(c, user, &[0])?.into_iter().filter(|o| o.exchange == broker).collect();
    Ok(match usable.as_slice() {
        [one] if one.number.is_some() => vec![(one.account, one.number)],
        [] | [_] => vec![(None, None)],
        many => many.iter().map(|o| (o.account, o.number)).collect(),
    })
}

fn page(c: &Connection, ctx: &Ctx, user: i64, config: Option<&Value>, brokers: Vec<(i64, String)>) -> Result<String, WebError> {
    let order = StepOrder::new(BotType::Multi, current_variant(config));
    let mut venues = vec![];
    for (id, class) in brokers {
        for (account, number) in tiles(c, user, id)? {
            venues.push(Venue { id, account, number, mark: exchange_wordmark(&crate::tracker::rows::name_id(&class)), unavailable: false });
        }
    }
    let table = Table { action: ctx.path("/bots/dca_multi_assets/pick_stock_broker"), csrf: ctx.csrf_token(), param_name: FIELD.into(), venues,
                        unavailable_note: ctx.t("bot.hyperliquid_trading_unavailable") };
    // 2.x draws no sentence here: the track says where the flow stands and does nothing (D-WIZARD-TRACK r5).
    let chips = super::chips(c, &basket_ids(config))?;
    let stacks = super::grouped::stacks(c, ctx, config)?;
    let steered = super::steer(ctx, "", super::SentenceOf { order: &order, step: Step::Exchange, multi: true, chips, stacks, exchange: None }, false)?;
    super::sheet(ctx, &steered.track, &New { table }.render()?)
}

/// What the step answers, decided with the session as it stands.
enum Answer { Repick, Missing, Page(String, Shell), Chosen(i64, Option<i64>, super::Picked) }

async fn answer(app: &App, ctx: &Ctx, chosen: Option<String>) -> Result<Answer, WebError> {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("the broker step without a user".into()))?;
    let config = ctx.session.lock().bot_config.clone();
    let (inner, view) = (app.clone(), ctx.clone());
    app.db(move |c| {
        if !stock_bot(c, config.as_ref())? { return Ok(Answer::Repick) }
        let brokers = brokers(c, &inner, config.as_ref())?;
        if chosen.is_none() && brokers.is_empty() { return Ok(Answer::Repick) }
        if let Some(id) = chosen.as_deref().and_then(|wanted| brokers.iter().find(|(id, _)| id.to_string() == wanted).map(|(id, _)| *id)) {
            let Some(picked) = super::picked(c, &view, user.id, id)? else { return Ok(Answer::Missing) };
            return Ok(Answer::Chosen(id, super::usd_asset(c)?, picked));
        }
        Ok(Answer::Page(page(c, &view, user.id, config.as_ref(), brokers)?, Shell::load(c, &inner, &user)?))
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
        Answer::Missing => Ok(layout::missing()),
        Answer::Chosen(broker, usd, picked) => {
            finalize(ctx.session.lock().bot_config.get_or_insert_with(|| json!({})), broker, usd, picked);
            Ok(layout::redirect(StatusCode::FOUND, &ctx.path("/bots/dca_multi_assets/add_api_key/new")))
        }
    }
}

/// Bots::StockBrokerRoutable#finalize_stock_broker!: the broker's id, and the USD quote deep-merged into the settings.
pub fn finalize(config: &mut Value, broker: i64, usd: Option<i64>, picked: super::Picked) {
    if !config.is_object() { *config = json!({}); }
    config["exchange_id"] = json!(broker);
    super::keep_pick(config, picked);
    if let Some(usd) = usd {
        let settings = &mut config["settings"];
        if !settings.is_object() { *settings = json!({}); }
        settings["quote_asset_id"] = json!(usd);
    }
}
