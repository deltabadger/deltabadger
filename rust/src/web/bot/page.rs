//! `bots#show` and `bots/charts#show`: the page of one bot, and the frame its chart arrives in.
use super::{exchange_svg, orders, refusal, settings, status, Bot, Kind};
use crate::enums::BotStatus;
use crate::web::auth::{self, User};
use crate::web::bots::{json, market_data, Segmented, SegmentedOption};
use crate::web::layout::{self, Ctx, Page};
use crate::web::shell::{self, Shell};
use crate::web::{flash, header_text, i18n, App, WebError};
use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{Connection, OptionalExtension};

struct OtherExchange {
    id: i64,
    /// `exchange.name.capitalize`.
    name: String,
    svg: &'static str,
    /// The user has a working trading key there.
    connected: bool,
}

#[derive(Template)]
#[template(path = "bots/_chart.html")]
struct Chart<'a> {
    v: &'a Ctx,
    /// `bot.transactions.any?`
    charted: bool,
    hide_money: bool,
    modes: String,
}

#[derive(Template)]
#[template(path = "bots/chart.html")]
struct ChartFrame<'a> {
    args: String,
    chart: &'a str,
}

#[derive(Template)]
#[template(path = "bots/show.html")]
struct Show<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    preferences: &'a str,
    bot_updates: &'a str,
    page_stream: &'a str,
    /// `bot_path(bot)`.
    path: String,
    label_id: String,
    label: &'a str,
    other_bots: Vec<(String, String)>,
    menu: String,
    exchange_select: String,
    status_bar: String,
    status_button: String,
    chart: String,
    columns_id: String,
    working: bool,
    settings: String,
    hide_money: bool,
    order_filter: String,
    order_filters_id: String,
    default_filter: &'static str,
    order_filters: Option<String>,
    export_id: String,
    import_id: String,
    import_form_id: String,
    has_waiting: bool,
    bot_args: String,
    feed_empty: bool,
}

/// `[bot, :bot_updates]` as Turbo names it: the bot's GlobalID parameter, then the suffix.
pub fn page_stream(bot: &Bot) -> String {
    let class = match bot.kind { Kind::Basket => "Bots::DcaMultiAsset", Kind::Index => "Bots::DcaIndex" };
    format!("{}:bot_updates", URL_SAFE_NO_PAD.encode(format!("gid://deltabadger/{class}/{}", bot.id)))
}

/// `ActiveSupport::Inflector#capitalize`: the first letter up, the rest down.
fn capitalize(name: &str) -> String {
    let mut characters = name.chars();
    characters.next().map(|first| first.to_uppercase().chain(characters.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
}

/// Each type's `available_exchanges_for_current_settings`, without the bot's own venue.
fn other_exchanges(c: &Connection, bot: &Bot, user_id: i64) -> Result<Vec<OtherExchange>, WebError> {
    let quote = bot.setting("quote_asset_id").and_then(serde_json::Value::as_i64);
    let listed = "FROM tickers t JOIN exchanges e ON e.id = t.exchange_id WHERE t.available = 1 AND t.trading_enabled = 1 AND e.available = 1 AND (?1 IS NULL OR t.quote_asset_id = ?1)";
    let ids: Vec<i64> = match bot.kind {
        // Any venue that lists something in this quote.
        Kind::Index => c.prepare(&format!("SELECT DISTINCT t.exchange_id {listed} ORDER BY t.exchange_id"))?.query_map([quote], |r| r.get(0))?.collect::<Result<_, _>>()?,
        // A venue a user can still pick, listing every member in the same quote. Counted by the
        // database, as Bots::DcaMultiAsset#eligible_pairs counts it: the catalogue is never read into
        // memory, however large it is, and this runs inside the one database call the server has.
        Kind::Basket => {
            let members: Vec<i64> = bot.allocations().iter().map(|(id, _)| *id).collect();
            let tradeable = format!("{listed} AND e.type IS NOT 'Exchanges::Bitmart'");
            if members.is_empty() {
                c.prepare(&format!("SELECT DISTINCT t.exchange_id {tradeable} ORDER BY t.exchange_id"))?.query_map([quote], |r| r.get(0))?.collect::<Result<_, _>>()?
            } else {
                let sql = format!("SELECT DISTINCT exchange_id FROM (SELECT t.exchange_id AS exchange_id {tradeable} AND t.base_asset_id IN (SELECT value FROM json_each(?2)) \
                                   GROUP BY t.exchange_id, t.quote_asset_id HAVING COUNT(DISTINCT t.base_asset_id) = ?3) ORDER BY exchange_id");
                c.prepare(&sql)?.query_map((quote, serde_json::json!(members).to_string(), members.len() as i64), |r| r.get(0))?.collect::<Result<_, _>>()?
            }
        }
    };
    let mut others = vec![];
    for id in ids.into_iter().filter(|id| *id != bot.exchange.id) {
        let (name, class): (Option<String>, Option<String>) = c.query_row("SELECT name, type FROM exchanges WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let exchange = super::Exchange { id, name: name.unwrap_or_default(), class: class.unwrap_or_default(), maker_fee: None };
        let connected = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND status = 1)", [user_id, id], |r| r.get(0))?;
        others.push(OtherExchange { id, name: capitalize(&exchange.name), svg: exchange_svg(&exchange.name_id()), connected });
    }
    Ok(others)
}

/// The chart's widget while it waits, with the mode switch it will have held in place (`shared/_segmented`).
fn chart(ctx: &Ctx, bot: &Bot, hide_money: bool) -> Result<String, WebError> {
    let t = |key: &str| i18n::text(ctx.locale, key, &[]);
    let modes = Segmented {
        fluid: true, label: t("bot.details.stats.chart.modes"), key: None, links: false,
        options: vec![SegmentedOption { value: "pnl", label: t("bot.details.stats.chart.pnl"), active: true, href: None },
                      SegmentedOption { value: "value", label: t("bot.details.stats.chart.value"), active: false, href: None }],
    }.render()?;
    Ok(Chart { v: ctx, charted: bot.has_orders, hide_money, modes }.render()?)
}

pub(super) enum Found {
    Bot(Box<Bot>),
    /// Not this user's, or deleted: `Bots::Botable#set_bot` sends the browser to the list.
    Missing,
    NotPorted(&'static str),
}

/// The bot of a request: the signed-in user's and one this build renders. The page treats a deleted
/// bot as missing (Bots::Botable#set_bot); the chart's frame finds it, as `current_user.bots.find` does.
pub(super) fn find(c: &Connection, app: &App, user: &User, segment: &str, deleted_too: bool, asked: super::For) -> Result<(Found, bool), WebError> {
    let (deltabadger, configured) = market_data(c, app)?;
    let Some(id) = super::id_from_path(segment) else { return Ok((Found::Missing, configured)) };
    let status: Option<i64> = c.query_row("SELECT status FROM bots WHERE id = ?1 AND user_id = ?2", [id, user.id], |r| r.get(0)).optional()?;
    match status {
        None => return Ok((Found::Missing, configured)),
        Some(3) if !deleted_too => return Ok((Found::Missing, configured)),
        Some(_) => {}
    }
    let wash_sale: Option<bool> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [user.id], |r| r.get(0))?;
    if let Some(reason) = refusal(c, id, wash_sale, deltabadger, asked)? { return Ok((Found::NotPorted(reason), configured)); }
    let found = match Bot::find(c, user.id, id, asked)? {
        None => Found::Missing,
        Some(bot) => bot.unrendered().map_or_else(|| Found::Bot(Box::new(bot)), Found::NotPorted),
    };
    Ok((found, configured))
}

/// `redirect_to bots_path, alert: t('bot.not_found')`.
pub(super) fn not_found(ctx: &Ctx) -> Response {
    flash::set(&ctx.session, flash::ALERT, i18n::text(ctx.locale, "bot.not_found", &[]));
    layout::redirect(StatusCode::FOUND, &ctx.path("/bots"))
}

/// Whether Rails reads the request's format as a Turbo stream: the path says `.turbo_stream`, or
/// there is no extension and the Accept header, when it is not a browser's catch-all, names it first.
fn turbo_stream_format(extension: Option<&str>, headers: &HeaderMap) -> bool {
    match extension {
        Some(extension) => extension == "turbo_stream",
        None => header_text(headers, "accept").is_some_and(|accept| {
            let browser_like = accept.contains("*/*") && accept.contains(',');
            !browser_like && accept.split(',').next().is_some_and(|first| first.trim().starts_with("text/vnd.turbo-stream.html"))
        }),
    }
}

/// GET /bots/:id, and /bots/:id.turbo_stream, the orders feed's frame.
pub async fn show(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (id_part, extension) = segment.split_once('.').map_or((segment.as_str(), None), |(id, extension)| (id, Some(extension)));
    let feed = turbo_stream_format(extension, &headers) && ctx.turbo_frame.as_deref() == Some("orders_pagination");
    // Whose bot it is comes first: neither a redirect nor a refusal renders a form, so neither gives the session a token.
    let (inner, owner, id_part) = (app.clone(), user.clone(), id_part.to_string());
    let asked = if feed { super::For::Feed } else { super::For::Page };
    let (bot, configured) = match app.db(move |c| find(c, &inner, &owner, &id_part, false, asked)).await {
        Ok((Found::Bot(bot), configured)) => (*bot, configured),
        Ok((Found::Missing, _)) => return Ok(not_found(&ctx)),
        Ok((Found::NotPorted(reason), _)) => return Ok(layout::refused(&ctx, reason)),
        Err(error) => return layout::or_refused(&ctx, error),
    };
    if feed {
        let (view, owner) = (ctx.clone(), user.clone());
        let body = match app.db(move |c| orders::feed(c, &view, &bot, &owner)).await {
            Ok(Ok(body)) => body,
            Ok(Err(reason)) => return Ok(layout::refused(&ctx, reason)),
            Err(error) => return layout::or_refused(&ctx, error),
        };
        return Ok((StatusCode::OK, [(axum::http::header::CONTENT_TYPE, crate::web::turbo::CONTENT_TYPE)], body).into_response());
    }
    let csrf = ctx.csrf_token();
    let (inner, view, token, owner) = (app.clone(), ctx.clone(), csrf.clone(), user.clone());
    let page = app.db(move |c| {
        let (ctx, csrf, user) = (&view, token.as_str(), &owner);
        let shell = Shell::load(c, &inner, user)?;
        let path = ctx.path(&format!("/bots/{}", bot.id));
        let mut others = c.prepare("SELECT id, label FROM bots WHERE user_id = ?1 AND status NOT IN (3, 7) AND id != ?2 ORDER BY position, id")?;
        let other_bots = others.query_map([user.id, bot.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?
            .map(|row| row.map(|(id, label)| (ctx.path(&format!("/bots/{id}")), label.unwrap_or_default()))).collect::<Result<Vec<_>, _>>()?;
        let hide_money = user.hide_balances;
        let status = status::render(c, ctx, csrf, &bot, configured)?;
        let forms = settings::Forms { ctx, csrf, bot: &bot, path: path.clone(), hide_balances: hide_money, time_zone: &user.time_zone, check: status.check.as_ref() };
        // BotHelper#order_filter_tabs: [value, available?], and the tab the log opens on.
        let exists = |sql: &str| -> Result<bool, WebError> { Ok(c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {sql})"), [bot.id], |r| r.get(0))?) };
        let activities = exists("bot_activity_logs WHERE bot_id = ?1 AND event NOT IN ('order_skipped', 'order_ignored')")?;
        let mut tabs: Vec<(&'static str, &'static str, bool)> = vec![];
        if !hide_money { tabs.push(("all", "order_filters.all", true)); }
        tabs.push(("successful", "order_filters.transactions", exists("transactions WHERE bot_id = ?1 AND status = 0 AND external_status = 2")?));
        tabs.push(("waiting", "order_filters.waiting", bot.has_waiting_orders));
        tabs.push(("other", "order_filters.other", exists("transactions WHERE bot_id = ?1 AND (external_status IN (3, 4) OR status IN (1, 2))")? || (hide_money && activities)));
        let default_filter = tabs.iter().find(|(_, _, available)| *available).map_or("successful", |(value, _, _)| *value);
        let order_filter = ctx.params.query("order_filter").unwrap_or(default_filter).to_string();
        let order_filters = if tabs.iter().filter(|(value, _, available)| *available && *value != "all").count() > 1 {
            let options = tabs.iter().filter(|(_, _, available)| *available).map(|(value, label, _)| SegmentedOption { value, label: i18n::text(ctx.locale, label, &[]), active: order_filter == *value, href: None }).collect();
            Some(Segmented { fluid: true, label: i18n::text(ctx.locale, "order_filters.all", &[]), key: None, options, links: false }.render()?)
        } else {
            None
        };
        let (preferences, bot_updates, page_stream) = (format!("user_{}:preferences", user.id), format!("user_{}:bot_updates", user.id), page_stream(&bot));
        let body = Show {
            v: ctx, csrf, preferences: &preferences, bot_updates: &bot_updates, page_stream: &page_stream, path: path.clone(),
            label_id: bot.dom_id("label"), label: &bot.label, other_bots,
            menu: menu(ctx, csrf, &bot, configured)?, exchange_select: exchange_select(c, ctx, csrf, &bot, user.id)?,
            status_bar: status.bar, status_button: status.button, chart: chart(ctx, &bot, hide_money)?, columns_id: bot.dom_id("columns"), working: bot.working(),
            settings: settings::column(c, &forms)?, hide_money, order_filter, order_filters_id: bot.dom_id("order_filters"), default_filter, order_filters,
            export_id: bot.dom_id("export"), import_id: bot.dom_id("import"), import_form_id: bot.dom_id("import_form"), has_waiting: bot.has_waiting_orders,
            bot_args: json(&serde_json::json!({ "bot_id": bot.id })), feed_empty: !bot.has_orders && !activities,
        }.render()?;
        Ok((shell, body))
    }).await;
    // The sums under a spending cap are read while the page is built: an amount this build does not read refuses the page there.
    let (shell, body) = match page { Ok(page) => page, Err(error) => return layout::or_refused(&ctx, error) };
    shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: Vec::new() })
}

/// GET /bots/:bot_id/chart: the chart's frame. A bot that is not this user's is a 404: `find` raises in Rails.
pub async fn chart_frame(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, owner) = (app.clone(), user.clone());
    let bot = match app.db(move |c| find(c, &inner, &owner, &segment, true, super::For::Page)).await {
        Ok((Found::Bot(bot), _)) => *bot,
        Ok((Found::Missing, _)) => return Ok(layout::missing()),
        Ok((Found::NotPorted(reason), _)) => return Ok(layout::refused(&ctx, reason)),
        Err(error) => return layout::or_refused(&ctx, error),
    };
    let csrf = ctx.csrf_token();
    let chart = chart(&ctx, &bot, user.hide_balances)?;
    let body = ChartFrame { args: json(&serde_json::json!({ "bot_id": bot.id })), chart: &chart }.render()?;
    let (inner, owner) = (app.clone(), user.clone());
    let shell = match app.db(move |c| Shell::load(c, &inner, &owner)).await { Ok(shell) => shell, Err(error) => return layout::or_refused(&ctx, error) };
    shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: Vec::new() })
}

#[derive(Template)]
#[template(path = "bots/_exchange_select.html")]
struct ExchangeSelect<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    path: String,
    param_key: &'static str,
    exchange_name_id: String,
    exchange_svg: &'static str,
    exchange_name: &'a str,
    switchable: bool,
    can_reconnect: bool,
    exchange_note: Option<String>,
    other_exchanges: Vec<OtherExchange>,
    needs_key: bool,
}

pub(super) fn exchange_select(c: &Connection, ctx: &Ctx, csrf: &str, bot: &Bot, owner: i64) -> Result<String, WebError> {
        let switchable = !bot.working() && !bot.rebalance_pending();
        let other_exchanges = if switchable { other_exchanges(c, bot, owner)? } else { vec![] };
        let exchange_note = if bot.rebalance_pending() {
            Some(i18n::text(ctx.locale, "bot.exchange_menu.locked_while_rebalancing", &[]))
        } else if !switchable {
            Some(i18n::text(ctx.locale, "bot.exchange_menu.locked_while_running", &[]))
        } else if other_exchanges.is_empty() {
            // `bot.assets.pluck(:symbol).to_sentence`: the quote, and for a basket its members.
            let mut symbols: Vec<String> = bot.quote_asset.iter().map(|asset| asset.symbol().to_string()).collect();
            symbols.extend(bot.base_assets.iter().map(|asset| asset.symbol().to_string()));
            Some(i18n::text(ctx.locale, "bot.exchange_menu.no_alternative", &[("assets", i18n::Arg::Text(&crate::ruby::to_sentence(&symbols)))]))
        } else {
            None
        };
    Ok(ExchangeSelect { v: ctx, csrf, path: ctx.path(&format!("/bots/{}",bot.id)), param_key: bot.param_key(),
        exchange_name_id: bot.exchange.name_id(), exchange_svg: exchange_svg(&bot.exchange.name_id()), exchange_name: &bot.exchange.name,
        switchable, can_reconnect: switchable && bot.api_key_correct() && !bot.exchange.retired(), exchange_note, other_exchanges,
        needs_key: !bot.exchange.retired() && !bot.api_key_correct() }.render()?)
}

#[derive(Template)]
#[template(source = r##"<div class="flex-row" id="{{ menu_id }}">
  <div data-controller="dropdown" data-action="click->dropdown#toggle" data-dropdown-target="wrapper" class="sbutton sbutton--link dropdown-wrapper dropdown-wrapper--right">
    {% include "svg/_24x24_dot_menu.html" %}
    <div class="dropdown">
      <a class="dropdown__item" data-turbo-frame="modal" href="{{ path }}/edit">{{ v.t("utils.change_name")|safe }}</a>
{% if !archived %}
{% if index %}
          <a class="dropdown__item" data-turbo-frame="modal" href="{{ path }}/index/new">{{ v.t("bot.index_switch.change_index")|safe }}</a>
          <div data-controller="converting" data-action="turbo:submit-start->converting#show turbo:submit-end->converting#hide">
            <template data-converting-target="template">
  <dialog class="dialog dialog--open">
    <div class="dialog__vertical-center">
      <div class="modal modal--busy">
        <div class="loader"></div>
        <p>{{ v.t("bot.index_switch.converting")|safe }}</p>
      </div>
    </div>
  </dialog>
</template>

            <form class="button_to" method="post" action="{{ path }}/custom_allocation"><button class="dropdown__item" type="submit">{{ v.t("bot.index_switch.custom_allocation")|safe }}</button><input type="hidden" name="authenticity_token" value="{{ csrf }}" /></form>
</div>{% else if follow_index %}
          <a class="dropdown__item" data-turbo-frame="modal" href="{{ path }}/index/new">{{ v.t("bot.index_switch.follow_index")|safe }}</a>
{% endif %}
        <a class="dropdown__item" data-turbo-frame="modal" href="{{ path }}/archive/edit">{{ v.t("button.archive")|safe }}</a>
{% endif %}
      <div class="dropdown__item dropdown__item--divider"></div>
      <a class="dropdown__item dropdown__item--danger" data-turbo-frame="modal" href="{{ path }}/delete/edit">{{ v.t("button.delete")|safe }}</a>
    </div>
  </div>
</div>"##, ext = "html")]
struct Menu<'a> { v: &'a Ctx, csrf: &'a str, path: String, menu_id: String, archived: bool, index: bool, follow_index: bool }

pub(super) fn menu(ctx: &Ctx, csrf: &str, bot: &Bot, configured: bool) -> Result<String, WebError> {
    Ok(Menu { v: ctx, csrf, path: ctx.path(&format!("/bots/{}",bot.id)), menu_id: bot.dom_id("menu"),
        archived: bot.status == BotStatus::Archived, index: bot.kind == Kind::Index, follow_index: bot.kind == Kind::Basket && configured }.render()?)
}
