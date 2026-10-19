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
use rusqlite::Connection;
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
        EngineEvent::StatusChanged { bot_id } | EngineEvent::TickEnded { bot_id } | EngineEvent::OrderRecorded { bot_id, .. } | EngineEvent::OrderWritten { bot_id, .. } => *bot_id,
        _ => return Ok(vec![]),
    };
    let owner: Option<i64> = rusqlite::OptionalExtension::optional(c.query_row("SELECT user_id FROM bots WHERE id = ?1", [bot_id], |r| r.get(0)))?;
    let Some(owner) = owner.and_then(|id| User::find(c, id).transpose()).transpose()? else { return Ok(vec![]) };
    let ctx = owner_ctx(app, owner.clone());
    // A bot this build does not render, or deleted meanwhile: Rails' job finds nothing to draw either.
    let Some(bot) = Bot::find(c, owner.id, bot_id, For::Page, ctx.locale)? else { return Ok(vec![]) };
    if bot.unrendered().is_some() { return Ok(vec![]); }
    let stream = format!("user_{}:bot_updates", owner.id);
    let state = || -> Result<status::Status, WebError> { status::broadcast(c, &ctx, &bot, bots::market_data(c, app)?.1) };
    Ok(match event {
        EngineEvent::StatusChanged { .. } => {
            let state = state()?;
            let mut out = action_view::bar_broadcasts(&stream, &bot, &state).to_vec();
            out.extend(action_view::button_broadcasts(&stream, &bot, &state));
            out
        }
        EngineEvent::TickEnded { .. } => {
            let state = state()?;
            let mut out = action_view::bar_broadcasts(&stream, &bot, &state).to_vec();
            if !bot.working() { out.extend(action_view::button_broadcasts(&stream, &bot, &state)); }
            out
        }
        EngineEvent::OrderRecorded { transaction_id, .. } => orders::new_order(c, &ctx, &bot, &owner, *transaction_id)?,
        EngineEvent::OrderWritten { transaction_id, .. } => orders::updated_order(c, &ctx, &bot, &owner, *transaction_id)?,
        _ => vec![],
    })
}

/// The `live` service of `deltabadger serve`: each event the engine sends (EngineEvent::is_live) is rendered and broadcast in
/// the order it was sent. A render that fails is logged and dropped, as a Rails broadcast job that raised is; the next
/// event, or a reload, shows the bot as it is.
pub async fn follow(app: App, mut events: tokio::sync::mpsc::UnboundedReceiver<EngineEvent>, mut stop: tokio::sync::watch::Receiver<bool>) -> Result<(), String> {
    loop {
        tokio::select! {
            _ = stop.wait_for(|stopped| *stopped) => return Ok(()),
            event = events.recv() => {
                let Some(event) = event else { return Ok(()) };
                let inner = app.clone();
                let shown = event.clone();
                match app.db(move |c| render(c, &inner, &event)).await {
                    Ok(messages) => for (stream, html) in messages { app.hub.broadcast(&stream, &html); },
                    Err(err) => crate::engine::log(&format!("[web] {shown:?}: the bot page broadcast could not be rendered: {err:?}")),
                }
            }
        }
    }
}
