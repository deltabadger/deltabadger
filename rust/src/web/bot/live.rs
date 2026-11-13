//! What the bot page hears while the engine works on the bot: the broadcasts Rails makes from its jobs and callbacks,
//! rendered as the owner sees them (Bot#with_user_locale) from the rows as they are when the event is read. A status the
//! engine changed (a tick's claim, a follow-up fill's amount-limit stop) redraws the status as Rails' after_update_commit
//! does (app/models/bot.rb:38-40); a tick's end redraws the
//! bars, as Bot::ActionJob's rescue and Bot::BroadcastAfterScheduledActionJob do, and the buttons and columns lock too when
//! the tick stopped the bot (Bot::StopJob's status change); each order inserted or written is Transaction's
//! after_create_commit / after_update_commit (Bot#broadcast_new_order, #broadcast_updated_order).
use super::{action_view, orders, status, Bot, For};
use crate::engine::events::EngineEvent;
use crate::web::auth::{Current, User};
use crate::web::layout::Ctx;
use crate::web::{bots, App, WebError};
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;

/// A request-less Ctx for `owner`: their locale and its paths, no session.
pub(super) fn owner_ctx(app: &App, owner: User) -> Ctx {
    let locale = owner.locale.as_deref().and_then(crate::web::locale::known).unwrap_or("en");
    Ctx { app: app.clone(), params: Arc::new(crate::web::Params { full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None,
            query: vec![], form: vec![], json: None, upload: None }),
          session: crate::web::session::Session::new(Default::default()), current: Current::SignedIn(Arc::new(owner)),
          method: axum::http::Method::GET, locale, nonce: String::new(), now: app.now(), turbo_frame: None, stream: false }
}

/// The messages one event owes the bot's page, in Rails' order.
pub fn render(c: &Connection, app: &App, event: &EngineEvent) -> Result<Vec<(String, String)>, WebError> {
    let bot_id = match event {
        EngineEvent::StatusChanged { bot_id } | EngineEvent::TickEnded { bot_id } | EngineEvent::OrderRecorded { bot_id, .. } | EngineEvent::OrderWritten { bot_id, .. }
        | EngineEvent::SignalMarkerChanged { bot_id } | EngineEvent::Reversed { bot_id } | EngineEvent::ConditionInfo { bot_id, .. } => *bot_id,
        _ => return Ok(vec![]),
    };
    let owner: Option<i64> = OptionalExtension::optional(c.query_row("SELECT user_id FROM bots WHERE id = ?1", [bot_id], |r| r.get(0)))?;
    let Some(owner) = owner.and_then(|id| User::find(c, id).transpose()).transpose()? else { return Ok(vec![]) };
    let ctx = owner_ctx(app, owner.clone());
    // A bot this build does not render, or deleted meanwhile: Rails' job finds nothing to draw either.
    let Some(bot) = Bot::find(c, owner.id, bot_id, For::Page, ctx.locale)? else { return Ok(vec![]) };
    if bot.unrendered().is_some() { return Ok(vec![]); }
    let stream = format!("user_{}:bot_updates", owner.id);
    let state = || -> Result<status::Status, WebError> { status::broadcast(c, &ctx, &bot, bots::market_data(c, app)?.1) };
    let mut out = match event {
        EngineEvent::StatusChanged { .. } => {
            let state = state()?;
            let mut out = action_view::bar_broadcasts(&stream, &bot, &state).to_vec();
            out.extend(action_view::button_broadcasts(&stream, &bot, &state));
            out
        }
        EngineEvent::Reversed { .. } => {
            let Some(draft) = super::draft::Draft::load(c, owner.id, bot_id, ctx.locale)? else { return Ok(vec![]) };
            let settings = super::settings::draft_column(c, &ctx, "", &draft, &owner.time_zone, owner.hide_balances)?;
            action_view::broadcast_settings_metrics(&ctx, &bot, true);
            vec![(super::page::page_stream(&bot), crate::web::turbo::stream("replace", "settings", &settings))]
        }
        EngineEvent::TickEnded { .. } => {
            let state = state()?;
            let mut out = action_view::bar_broadcasts(&stream, &bot, &state).to_vec();
            if !bot.working() { out.extend(action_view::button_broadcasts(&stream, &bot, &state)); }
            out
        }
        EngineEvent::OrderRecorded { transaction_id, .. } => orders::new_order(c, &ctx, &bot, &owner, *transaction_id)?,
        EngineEvent::OrderWritten { transaction_id, .. } => {
            let mut out = orders::updated_order(c, &ctx, &bot, &owner, *transaction_id)?;
            // D-PENDING-MARKET-SETTING: a market order written after its run (a follow-up poll, a settled zero fill) may end the
            // "Setting orders…" the bar held while it was pending; the bar is redrawn as after a run.
            let market: bool = c.query_row("SELECT IFNULL(order_type = 0, 0) FROM transactions WHERE id = ?1", [transaction_id], |r| r.get(0)).optional()?.unwrap_or(false);
            if market { out.extend(action_view::bar_broadcasts(&stream, &bot, &state()?)); }
            // Bot::BaseAmountLimitable#handle_base_amount_limit_update: a sell written while the bot sells its one asset under
            // the base cap redraws what is left of it, before the stop its fill may cause (StatusChanged follows).
            let sell: bool = c.query_row("SELECT IFNULL(side = 1, 0) FROM transactions WHERE id = ?1", [transaction_id], |r| r.get(0)).optional()?.unwrap_or(false);
            if sell && bot.selling() && bot.one_asset() && bot.on("base_amount_limited") {
                let info = super::settings::base_amount_limit_info(c, ctx.locale, &bot, owner.hide_balances, ctx.now)?;
                out.push((super::page::page_stream(&bot), crate::web::turbo::stream("replace", "settings-amount-limit-info", &info)));
            }
            out
        }
        // Bots::Signal#broadcast_unresolved_placement: on the bot's own page, without the rule widgets (their webhook URL
        // needs a request).
        EngineEvent::SignalMarkerChanged { .. } if bot.kind == super::Kind::Signal =>
            vec![(super::page::page_stream(&bot), super::signal::unresolved_broadcast(&ctx, &bot))],
        // Bot#broadcast_price_limit_info_update: the info as its fetch left it, in the job's locale (the request's, else the
        // owner's), into the status line of that side's rule (D-INFO-SIDE: settings::info_target); nothing while that rule
        // is off (the partial's guard) or the page has no line for the side.
        EngineEvent::ConditionInfo { rule, side, locale, .. } => {
            use crate::engine::conditions::{INFO_KEY, SELLING};
            let prefix = if *side == SELLING { "sell_" } else { "" };
            let info = bot.transient.get(INFO_KEY).and_then(|v| v.get(*rule)).and_then(|v| v.get(*side));
            match (info, super::settings::info_target(&bot, rule, side)) {
                (Some(info), Some(target)) if bot.on(&format!("{prefix}{rule}ed")) => {
                    let locale = locale.as_deref().and_then(crate::web::locale::known).unwrap_or(ctx.locale);
                    vec![(super::page::page_stream(&bot), crate::web::turbo::stream("replace", &target, &super::settings::condition_info(locale, &bot, rule, *side == SELLING, info, &target)))]
                }
                _ => vec![],
            }
        }
        _ => vec![],
    };
    if matches!(event, EngineEvent::StatusChanged { .. } | EngineEvent::TickEnded { .. } | EngineEvent::OrderRecorded { .. } | EngineEvent::OrderWritten { .. }) {
        out.extend(selling_streams(c, &ctx, &bot, owner.hide_balances)?);
    }
    Ok(out)
}

/// The `live` service of `deltabadger serve`: each event the engine sends (EngineEvent::is_live), and each the web asks for
/// (`web`, App::announce), is rendered and broadcast one at a time, so what the page last receives is what was last read
/// (D-LIQUIDATION-ONE-LINE). A render that fails is logged and dropped, as a Rails broadcast job that raised is; the next
/// event, or a reload, shows the bot as it is.
pub async fn follow(app: App, mut events: tokio::sync::mpsc::UnboundedReceiver<EngineEvent>, mut web: Option<tokio::sync::mpsc::UnboundedReceiver<EngineEvent>>,
                    mut stop: tokio::sync::watch::Receiver<bool>) -> Result<(), String> {
    // The floor is read before the first event: a line that event's work wrote is above it.
    let lines = Arc::new(std::sync::Mutex::new(match app.db(Lines::starting).await {
        Ok(lines) => lines,
        Err(err) => { crate::engine::log(&format!("[web] activity watermark unread, the first event reads it: {err:?}")); Lines::default() }
    }));
    loop {
        let event = tokio::select! {
            _ = stop.wait_for(|stopped| *stopped) => return Ok(()),
            event = events.recv() => match event { Some(event) => event, None => return Ok(()) },
            Some(event) = async { match web.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => event,
        };
        let (inner, lines) = (app.clone(), lines.clone());
        let shown = event.clone();
        match app.db(move |c| {
            let mut lines = lines.lock().map_err(|_| WebError::Config("activity watermark poisoned".into()))?;
            // The marks move only with a batch that rendered whole: a failed one is tried again by the next event.
            let saved = lines.clone();
            // An order's older lines go below it, so they are prepended first; any other event's lines go on top.
            let batch = if matches!(event, EngineEvent::OrderRecorded { .. } | EngineEvent::OrderWritten { .. }) {
                lines.announce(c, &inner, &event).and_then(|mut out| { out.extend(render(c, &inner, &event)?); Ok(out) })
            } else {
                render(c, &inner, &event).and_then(|mut out| { out.extend(lines.announce(c, &inner, &event)?); Ok(out) })
            };
            if batch.is_err() { *lines = saved; }
            batch
        }).await {
            Ok(messages) => for (stream, html) in messages { app.hub.broadcast(&stream, &html); },
            Err(err) => crate::engine::log(&format!("[web] {shown:?}: the bot page broadcast could not be rendered: {err:?}")),
        }
    }
}

/// D-LIQUIDATION-ONE-LINE: the activity lines already on the bot pages. A bot not seen yet starts at the table's max id
/// read when the follower first renders; each live event prepends the bot's lines above its mark, as the feed selects them.
#[derive(Clone, Default)]
pub struct Lines { floor: Option<i64>, marks: std::collections::HashMap<i64, i64> }

impl Lines {
    /// Marks at the table's max id now.
    pub fn starting(c: &Connection) -> Result<Self, WebError> {
        Ok(Self { floor: Some(c.query_row("SELECT COALESCE(max(id), 0) FROM bot_activity_logs", [], |r| r.get(0))?), marks: Default::default() })
    }

    pub fn announce(&mut self, c: &Connection, app: &App, event: &EngineEvent) -> Result<Vec<(String, String)>, WebError> {
        let floor = match self.floor { Some(floor) => floor, None => *self.floor.insert(c.query_row("SELECT COALESCE(max(id), 0) FROM bot_activity_logs", [], |r| r.get(0))?) };
        let (EngineEvent::StatusChanged { bot_id } | EngineEvent::TickEnded { bot_id } | EngineEvent::OrderRecorded { bot_id, .. } | EngineEvent::OrderWritten { bot_id, .. }) = event else { return Ok(vec![]) };
        let mark = *self.marks.get(bot_id).unwrap_or(&floor);
        let owner: Option<i64> = OptionalExtension::optional(c.query_row("SELECT user_id FROM bots WHERE id = ?1", [bot_id], |r| r.get(0)))?;
        let Some(owner) = owner.and_then(|id| User::find(c, id).transpose()).transpose()? else { return Ok(vec![]) };
        let ctx = owner_ctx(app, owner.clone());
        let Some(bot) = Bot::find(c, owner.id, *bot_id, For::Page, ctx.locale)? else { return Ok(vec![]) };
        if bot.unrendered().is_some() { return Ok(vec![]); }
        // On an order's event only the lines older than the order (feed order: newest first, a line above an order of its instant).
        let before = match event {
            EngineEvent::OrderRecorded { transaction_id, .. } | EngineEvent::OrderWritten { transaction_id, .. } => {
                let at: Option<String> = OptionalExtension::optional(c.query_row("SELECT created_at FROM transactions WHERE id = ?1", [transaction_id], |r| r.get(0)))?;
                Some(at.map(|at| crate::codec::parse_time(&at).map_err(|e| super::data(format!("{e:?}")))).transpose()?.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC))
            }
            _ => None,
        };
        let (messages, last) = orders::new_lines(c, &ctx, &bot, &owner, mark, before)?;
        self.marks.insert(*bot_id, last.max(mark));
        Ok(messages)
    }
}

/// D-LIQUIDATION-ONE-LINE: a composition bot's "Selling …" rows and the state its Sell links read, as the page prints them
/// (`selling_html`), each replacing its target on the owner's stream.
pub fn selling_streams(c: &Connection, ctx: &Ctx, bot: &Bot, hidden: bool) -> Result<Vec<(String, String)>, WebError> {
    if bot.kind == super::Kind::Signal { return Ok(vec![]); }
    let stream = format!("user_{}:bot_updates", bot_owner(c, bot.id)?);
    let (rows, state) = selling_html(c, ctx, bot, hidden)?;
    let mut out = vec![(stream.clone(), crate::web::turbo::stream("replace", &format!("orders_selling_{}", bot.id), &rows)),
                       (stream.clone(), crate::web::turbo::stream("replace", &format!("selling_state_{}", bot.id), &state))];
    // With balances hidden a "Selling …" row may be what opens "Other" (page::filters), so the filters follow it.
    if hidden { out.push((stream, super::page::filters_broadcast(c, ctx, bot, hidden)?)); }
    Ok(out)
}

fn bot_owner(c: &Connection, id: i64) -> Result<i64, WebError> { Ok(c.query_row("SELECT user_id FROM bots WHERE id = ?1", [id], |r| r.get(0))?) }

/// The `<tbody>` of "Selling ETH…" rows (one per asset, before the feed's `orders_list`) and the hidden state element.
/// Empty strings for a signal bot.
pub fn selling_html(c: &Connection, ctx: &Ctx, bot: &Bot, hidden: bool) -> Result<(String, String), WebError> {
    use crate::web::i18n::{escape, text, Arg};
    if bot.kind == super::Kind::Signal { return Ok((String::new(), String::new())); }
    let selling = crate::engine::composition::selling(c, &crate::engine::model::load_bot(c, bot.id)?, ctx.now)?;
    let mut rows = String::new();
    for asset in &selling.assets {
        let symbol: String = c.query_row("SELECT COALESCE(symbol, name, '') FROM assets WHERE id = ?1", [asset], |r| r.get(0)).optional()?.unwrap_or_default();
        rows.push_str(&format!("\n  <tr class=\"text-inactive\" data-order-filter-target=\"row\" data-order-type=\"{}\">\n    <td scope=\"row\" class=\"table__when\">\
            <div class=\"loader--small\" style=\"position: unset; float: left;\"></div></td>\n    <td colspan=\"{}\" class=\"table__sentence\">{}</td>\n  </tr>",
            if hidden { "other" } else { "all" }, if hidden { 1 } else { 3 }, escape(&text(ctx.locale, "engine.liquidation_selling", &[("base", Arg::Text(&symbol))]))));
    }
    let tbody = format!("<tbody id=\"orders_selling_{}\">{rows}{}</tbody>", bot.id, if rows.is_empty() { "" } else { "\n" });
    let state = format!("<div id=\"selling_state_{}\" hidden data-bot--selling-target=\"state\" data-assets=\"{}\" data-blocked=\"{}\" data-label=\"{}\"></div>",
                        bot.id, escape(&serde_json::json!(selling.assets).to_string()), selling.blocked, escape(&text(ctx.locale, "bot.liquidation.selling", &[])));
    Ok((tbody, state))
}

/// D-LOW-FUNDS-STATUS: a key's synced balances became current or stopped being so (jobs::notifications): its owner's bots on
/// that venue redraw their bars as after a run (EngineEvent::TickEnded), so "Low funds" comes and goes without a reload.
pub fn balances_changed(app: &App, key_id: i64) {
    let app = app.clone();
    tokio::spawn(async move {
        let inner = app.clone();
        let read = app.db(move |c| {
            let mut s = c.prepare("SELECT b.id FROM bots b JOIN api_keys k ON k.user_id = b.user_id AND k.exchange_id = b.exchange_id WHERE k.id = ?1 ORDER BY b.id")?;
            let ids = s.query_map([key_id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
            for bot_id in ids { inner.announce(c, EngineEvent::TickEnded { bot_id }); }
            Ok(())
        }).await;
        if let Err(err) = read { crate::engine::log(&format!("[web] key {key_id}: the status bars could not be redrawn after its balances changed: {err:?}")); }
    });
}
