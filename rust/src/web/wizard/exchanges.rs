//! The DCA wizard's exchange step (Bots::Wizard::PickExchangesController), in the single namespace (before any asset, the
//! first step of the default exchange-first order) and the multi one (a basket's venue). The grid is bots/pick_exchanges/_table,
//! which the tracker's venue picker renders too.
use super::{basket_ids, chips, current_variant, dig, grouped, prerequisite, reset_downstream, steer, step_complete, BotType, SentenceOf, Step,
            StepOrder, BASE_KEY, QUOTE_KEY};
use crate::web::bot::search::{self, Basket};
use crate::web::bot::{exchange_wordmark, Exchange};
use crate::web::layout::{self, Ctx, Page};
use crate::web::shell::{self, Shell};
use crate::web::{auth, App, WebError};
use askama::Template;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::Connection;
use serde_json::{json, Value};

/// bots/pick_exchanges/_table: one button per venue, posting `param_name`.
pub struct Table {
    pub action: String,
    pub csrf: String,
    pub param_name: String,
    pub venues: Vec<Venue>,
    pub unavailable_note: String,
}

pub struct Venue {
    pub id: i64,
    pub mark: &'static str,
    /// `trading && !exchange.order_placement_available?`. Only Hyperliquid's adapter can lack order placement, and only in a
    /// Windows bundle without hyperliquid-rb; this build signs Hyperliquid orders itself, so none is shown disabled.
    pub unavailable: bool,
    /// #710 7a §2: a Connected tile's account (posted as `account_id`), and its number when the venue has 2+ live accounts.
    pub account: Option<i64>,
    pub number: Option<i64>,
}

/// An account the user holds a correct key of the step's kinds on (#710 7a §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connected { pub exchange: i64, pub account: Option<i64>, pub number: Option<i64> }

/// Exchange::STABLE_TYPES.
const STABLE_TYPES: [&str; 4] = ["Exchanges::Binance", "Exchanges::BinanceUs", "Exchanges::Coinbase", "Exchanges::Kraken"];

/// Bots::Searchable#filter_exchanges_by_query: stable venues first, then by name; a query keeps the venues whose name is 0.7
/// alike (JaroWinkler), beta ones last, the closest first.
pub fn ordered(venues: Vec<Exchange>, query: Option<&str>) -> Vec<Exchange> {
    let beta = |venue: &Exchange| !STABLE_TYPES.contains(&venue.class.as_str());
    match query.filter(|query| !crate::v2::blank(query)) {
        None => {
            let mut venues = venues;
            venues.sort_by(|a, b| beta(a).cmp(&beta(b)).then_with(|| a.name.as_bytes().cmp(b.name.as_bytes())));
            venues
        }
        Some(query) => {
            let query = search::downcase(query);
            let mut scored: Vec<(Exchange, f64)> = venues.into_iter().map(|venue| {
                let score = if crate::v2::blank(&venue.name) { 0.0 } else { search::jaro_winkler(&search::downcase(&venue.name), &query) };
                (venue, score)
            }).filter(|(_, score)| *score >= 0.7).collect();
            // ponytail: a stable sort; 2.x's sort_by is not, so two venues tied on beta and score may come out in another order.
            scored.sort_by(|(a, x), (b, y)| beta(a).cmp(&beta(b)).then_with(|| y.total_cmp(x)));
            scored.into_iter().map(|(venue, _)| venue).collect()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Namespace { Single, Multi }

impl Namespace {
    fn base(self) -> &'static str { if self == Namespace::Single { "/bots/dca_single_assets" } else { "/bots/dca_multi_assets" } }
    fn param(self) -> &'static str { if self == Namespace::Single { "bots_dca_single_asset[exchange_id]" } else { "bots_dca_multi_asset[exchange_id]" } }
    fn bot_type(self) -> BotType { if self == Namespace::Single { BotType::Single } else { BotType::Multi } }
}

/// The settings id at `key` as the bot built from the session reads it (nil unless present).
fn setting_id(config: Option<&Value>, key: super::Key) -> Option<i64> {
    dig(config, key).filter(|value| !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())).map(super::to_i)
}

/// Where a step that is not ready sends the browser back to.
fn prerequisite_path(c: &Connection, user_id: i64, config: Option<&Value>, namespace: Namespace) -> Result<Option<&'static str>, WebError> {
    let order = StepOrder::new(namespace.bot_type(), current_variant(config));
    if namespace == Namespace::Single && step_complete(c, user_id, config, Step::Assets)? {
        // This step comes before any asset: once a basket is chosen, re-picking the venue is the basket's exchange step.
        return Ok(Some("/bots/dca_multi_assets/pick_exchange/new"));
    }
    let Some(step) = prerequisite(c, user_id, config, &order, Step::Exchange)? else { return Ok(None) };
    Ok(Some(match (namespace, step) {
        (_, Step::Currencies | Step::Assets) => "/bots/dca_single_assets/pick_buyable_asset/new",
        (Namespace::Single, Step::Exchange) => "/bots/dca_single_assets/pick_exchange/new",
        (Namespace::Single, Step::Api) => "/bots/dca_single_assets/add_api_key/new",
        // WizardSteps#missing_exchange_path: a stock basket's venue is its broker.
        (Namespace::Multi, Step::Exchange) => {
            let stock: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE category = 'Stock' AND id IN (SELECT value FROM json_each(?1)))",
                                          [json!(basket_ids(config)).to_string()], |r| r.get(0))?;
            if stock { "/bots/dca_multi_assets/pick_stock_broker/new" } else { "/bots/dca_multi_assets/pick_exchange/new" }
        }
        (Namespace::Multi, Step::Api) => "/bots/dca_multi_assets/add_api_key/new",
        (_, Step::Spendable) => "/bots/dca_multi_assets/pick_spendable_asset/new",
    }))
}

#[derive(Template)]
#[template(path = "wizard/pick_exchange.html")]
struct New {
    table: Table,
    groups: Vec<Group>,
}

/// D-WIZARD-VENUE-GROUPS: the DCA exchange step lists the stock brokers under "Brokers" and the rest under "Exchanges", each
/// in the order the table had them; a group with no venue is not drawn.
pub struct Group { title: String, venues: Vec<Venue>, connected: bool }

/// The accounts `user` holds a correct key of one of `kinds` on (`api_keys.key_type`: 0 trading, 1 withdrawal, 2 read-only),
/// by exchange, then number. An install without the account tables names each exchange alone.
pub fn connected(c: &Connection, user: i64, kinds: &[i64]) -> rusqlite::Result<Vec<Connected>> {
    // #710 7b rule 10: a fenced key (its read names another account) is no correct key of its account.
    let mut statement = c.prepare(&format!("SELECT k.id, k.exchange_id FROM api_keys k WHERE k.user_id = ?1 AND k.status = 1 AND NOT {} AND k.key_type IN (SELECT value FROM json_each(?2)) ORDER BY k.id",
                                           crate::identity::fenced_sql(c, "k")?))?;
    let keys: Vec<(i64, i64)> = statement.query_map(rusqlite::params![user, json!(kinds).to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut out: Vec<(Connected, i64)> = vec![];
    for (key, exchange) in keys {
        let account = crate::identity::account_of_key(c, key)?;
        if out.iter().any(|(o, _)| o.exchange == exchange && o.account == account) { continue }
        let (number, order) = match account { Some(a) => (crate::identity::label(c, a)?.number, c.query_row("SELECT number FROM exchange_accounts WHERE id = ?1", [a], |r| r.get(0))?), None => (None, 0) };
        out.push((Connected { exchange, account, number }, order));
    }
    out.sort_by_key(|(o, order)| (o.exchange, *order));
    Ok(out.into_iter().map(|(o, _)| o).collect())
}

/// `prepare_step` and the page: the venues the bot built from the session could trade on, its own venue set aside.
/// PLAN index-as-ingredient slice 6: a mixed selection lists the venues `grouped::eligible` admits, any quote (a quote a
/// refused save left behind narrows nothing).
fn page(c: &Connection, ctx: &Ctx, config: Option<&Value>, namespace: Namespace) -> Result<String, WebError> {
    let mixed = grouped::mixed(config);
    let quote = setting_id(config, QUOTE_KEY).filter(|_| !mixed);
    let (venues, chips) = match namespace {
        // The one-asset bot holds a base only in a session an earlier release left open.
        Namespace::Single => {
            let base = setting_id(config, BASE_KEY);
            (search::available_exchanges(c, &Basket { exchange_id: None, quote_asset_id: quote, base_asset_ids: vec![] }, Some(base))?, chips(c, &base.into_iter().collect::<Vec<_>>())?)
        }
        Namespace::Multi => {
            let ids = basket_ids(config);
            (search::available_exchanges(c, &Basket { exchange_id: None, quote_asset_id: quote, base_asset_ids: ids.clone() }, None)?, chips(c, &ids)?)
        }
    };
    let mut venues = venues;
    if mixed {
        let mut admitted = vec![];
        for venue in venues { if grouped::session_eligible(c, &ctx.app, config, venue.id, None)? { admitted.push(venue); } }
        venues = admitted;
    }
    let stacks = grouped::stacks(c, ctx, config)?;
    let csrf = ctx.csrf_token();
    let order = StepOrder::new(namespace.bot_type(), current_variant(config));
    let steered = steer(ctx, &csrf, SentenceOf { order: &order, step: Step::Exchange, multi: namespace == Namespace::Multi, chips, stacks, exchange: None }, true)?;
    let table = Table { action: ctx.path(&format!("{}/pick_exchange", namespace.base())), csrf, param_name: namespace.param().into(), venues: vec![],
                        unavailable_note: ctx.t("bot.hyperliquid_trading_unavailable") };
    let connected = connected(c, ctx.user().map_or(0, |user| user.id), &[0])?;
    super::sheet(ctx, &steered.track, &venue_pane(ctx, table, venues, &connected)?)
}

/// A venue step's pane (D-WIZARD-VENUE-GROUPS, D-SHEET-EVERYWHERE): `search` over the venues, searched by the request's `query`
/// as `ordered` does, the stock brokers under "Brokers" and the rest under "Exchanges", each in that order; a group with no venue
/// is not drawn. `table` says where a pick posts (its own `venues` unused).
/// D-WIZARD-CONNECTED-GROUP (#710): the venues among them the user is `connected` to come first under "Connected", in that
/// order too, and stay in their own group below: either tile picks the venue.
/// The exchange step never searches (D-EXCHANGE-NO-SEARCH): the venues are few enough to see at once.
/// D-VENUE-TILE-ASKS-KEY (#710 7a §2): the Connected group draws one tile per account, posting its `account_id` (the number in
/// a corner with 2+ live accounts); the form says `untargeted`, so a Brokers/Exchanges tile always opens the key step.
pub fn venue_pane(ctx: &Ctx, table: Table, venues: Vec<Exchange>, connected: &[Connected]) -> Result<String, WebError> {
    let venues = ordered(venues, None);
    let venue = |venue: &Exchange| Venue { id: venue.id, mark: exchange_wordmark(&venue.name_id()), unavailable: false, account: None, number: None };
    let group = |key: &str, keep: &dyn Fn(&Exchange) -> bool| Group { title: ctx.t(&format!("bot.setup.venue_groups.{key}")), connected: key == "connected",
                                                                     venues: venues.iter().filter(|v| keep(v)).map(venue).collect() };
    let accounts = Group { title: ctx.t("bot.setup.venue_groups.connected"), connected: true,
                           venues: venues.iter().flat_map(|v| connected.iter().filter(|a| a.exchange == v.id).map(|a| Venue { account: a.account, number: a.number, ..venue(v) })).collect() };
    let groups: Vec<Group> = [accounts, group("brokers", &|v| super::assets::broker(&v.class)),
                              group("exchanges", &|v| !super::assets::broker(&v.class))].into_iter().filter(|group| !group.venues.is_empty()).collect();
    Ok(New { table, groups }.render()?)
}

async fn render(app: &App, ctx: &Ctx, namespace: Namespace, status: StatusCode) -> Result<Response, WebError> {
    let user = ctx.user().cloned().ok_or_else(|| WebError::Config("the exchange step without a user".into()))?;
    let config = ctx.session.lock().bot_config.clone();
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let (body, shell) = app.db(move |c| Ok((page(c, &view, config.as_ref(), namespace)?, Shell::load(c, &inner, &owner)?))).await?;
    shell::application(ctx, &ctx.csrf_token(), &user, &shell, Page { status, body, flash_now: vec![] })
}

async fn new(app: App, ctx: Ctx, namespace: Namespace) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let config = ctx.session.lock().bot_config.clone();
    if let Some(path) = app.db(move |c| prerequisite_path(c, user.id, config.as_ref(), namespace)).await? {
        return Ok(layout::redirect(StatusCode::FOUND, &ctx.path(path)));
    }
    render(&app, &ctx, namespace, StatusCode::OK).await
}

/// POST: a venue is stored as the form sent it, and the steps it and the later ones own are cleared first; a blank one
/// re-renders the step (422). The multi step needs a wizard begun; the single one is the default first step, where a POST with
/// no session yet is the normal first pick.
async fn create(app: App, ctx: Ctx, namespace: Namespace) -> Result<Response, WebError> {
    if ctx.user().is_none() { return Ok(auth::unauthenticated(&ctx)) }
    if namespace == Namespace::Multi { if let Some(answer) = super::expired(&ctx) { return Ok(answer) } }
    let Some(exchange) = ctx.params.form(namespace.param()).or_else(|| ctx.params.query(namespace.param())).map(str::to_string) else { return Ok(layout::bad_request()) };
    if crate::v2::blank(&exchange) { return render(&app, &ctx, namespace, StatusCode::UNPROCESSABLE_ENTITY).await }
    let (view, user, id) = (ctx.clone(), ctx.user().map_or(0, |u| u.id), super::to_i(&json!(exchange)));
    // A mixed selection takes only a venue `grouped::eligible` admits, from either namespace's form.
    let (inner, held) = (app.clone(), ctx.session.lock().bot_config.clone());
    if grouped::mixed(held.as_ref()) && !app.db(move |c| grouped::session_eligible(c, &inner, held.as_ref(), id, None)).await? {
        return render(&app, &ctx, namespace, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
    let Some(picked) = app.db(move |c| Ok(super::picked(c, &view, user, id)?)).await? else { return Ok(layout::missing()) };
    {
        let mut data = ctx.session.lock();
        let config = data.bot_config.get_or_insert_with(|| json!({}));
        if !config.is_object() { *config = json!({}); }
        let order = StepOrder::new(namespace.bot_type(), current_variant(Some(config)));
        reset_downstream(config, &order, Step::Exchange);
        config["exchange_id"] = json!(exchange);
        super::keep_pick(config, picked);
    }
    Ok(layout::redirect(StatusCode::FOUND, &ctx.path(&format!("{}/add_api_key/new", namespace.base()))))
}

pub async fn single_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { new(app, ctx, Namespace::Single).await }
pub async fn single_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { create(app, ctx, Namespace::Single).await }
pub async fn multi_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { new(app, ctx, Namespace::Multi).await }
pub async fn multi_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> { create(app, ctx, Namespace::Multi).await }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_venues_first_and_a_query_keeps_the_alike() {
        let venue = |id, name: &str, class: &str| Exchange { id, name: name.into(), class: class.into(), maker_fee: None, number: None };
        let all = || vec![venue(1, "Alpaca", "Exchanges::Alpaca"), venue(2, "Interactive Brokers", "Exchanges::Ibkr"), venue(3, "Kraken", "Exchanges::Kraken"),
                          venue(4, "Binance", "Exchanges::Binance")];
        let ids = |venues: Vec<Exchange>| venues.into_iter().map(|venue| venue.id).collect::<Vec<_>>();
        assert_eq!(ids(ordered(all(), None)), vec![4, 3, 1, 2]);
        assert_eq!(ids(ordered(all(), Some(" "))), vec![4, 3, 1, 2], "a blank query is no query");
        assert_eq!(ids(ordered(all(), Some("KRAK"))), vec![3]);
        assert!(ordered(all(), Some("zzzz")).is_empty());
    }
}
