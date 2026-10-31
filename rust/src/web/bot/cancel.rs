//! Bots::CancelOrdersController#destroy: cancel one open order from the bot page, inside the request, as Rails does
//! (PLAN-A4-cancel r4). The one exception to "no venue call in a request": ONE bounded, idempotent venue call for an
//! action whose answer the user is waiting for, and the request never writes order state. It sends the cancel, leaves the
//! engine a re-read (`REREAD`) and wakes it; the engine's follow-up poll settles the row.
use super::page;
use crate::web::{auth, flash, i18n, layout::{self, Ctx}, turbo, App, WebError};
use crate::venue::{Venue, VenueError, VenueFactory};
use rusqlite::OptionalExtension;
use axum::{extract::{Extension, Path, State}, http::StatusCode, response::{IntoResponse, Response}};

/// Rust-only: `rust_cancel_reread:{transaction id}` → `{"bot": id}`, a user cancel's re-read the engine owes
/// (engine::run::run_cancel_rereads). Never order state.
pub const REREAD: &str = "rust_cancel_reread:";

enum Found { NoBot, NoRow, Refused, Answered { cancelled: bool, invalid_key: Option<String> } }

pub async fn destroy(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((bot, tx)): Path<(String, String)>) -> Result<Response, WebError> {
    destroy_with_factory(State(app), Extension(ctx), Path((bot, tx)), |c, app| {
        // The App's Alpaca hosts, as MCP reads use them (a test's server; Alpaca's own otherwise).
        crate::venue::alpaca::LiveFactory::for_install(c, &app.cipher).with_clock(app.clock.clone()).on_hosts(app.settings_key_hosts.clone())
    }).await
}

/// The handler with an injected venue factory, for deterministic transport tests. Like the rebalance Resume it runs on a
/// connection of its own, so the web's shared connection is never held while the venue answers.
#[doc(hidden)]
pub async fn destroy_with_factory<F: VenueFactory>(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((bot, tx)): Path<(String, String)>,
    factory: impl FnOnce(&rusqlite::Connection, &App) -> F + Send + 'static) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let path = app.db(|c| c.path().filter(|path| !path.is_empty()).map(str::to_owned)
        .ok_or_else(|| WebError::Config("a cancel requires an install database".into()))).await?;
    let inner = app.clone();
    let runtime = tokio::runtime::Handle::current();
    let found = tokio::task::spawn_blocking(move || -> Result<Found, WebError> {
        let connection = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        crate::store::configure(&connection)?;
        let c = &connection;
        // Rails' before_actions, in order: Botable#set_bot, set_transaction; then Transaction#cancel.
        let Some(id) = super::id_from_path(&bot) else { return Ok(Found::NoBot) };
        let status: Option<i64> = c.query_row("SELECT status FROM bots WHERE id = ?1 AND user_id = ?2", (id, user.id), |r| r.get(0)).optional()?;
        if status.is_none_or(|s| s == crate::enums::BotStatus::Deleted as i64) { return Ok(Found::NoBot) }
        let tx = tx.strip_suffix(".turbo_stream").unwrap_or(&tx);
        let row: Option<(i64, Option<String>)> = match super::id_from_path(tx) {
            Some(tx) => c.query_row("SELECT id, external_id FROM transactions WHERE id = ?1 AND bot_id = ?2", (tx, id), |r| Ok((r.get(0)?, r.get(1)?))).optional()?,
            None => None,
        };
        let Some((tx, external)) = row else { return Ok(Found::NoRow) };
        // Transaction#cancel refuses an imported row; a row with no order id has nothing to name (D-CANCEL-NO-EXTERNAL-ID).
        let Some(ext) = external.filter(|e| !e.starts_with("imported_")) else { return Ok(Found::Refused) };
        let bot = crate::engine::model::load_bot(c, id)?;
        let name: String = c.query_row("SELECT coalesce(e.name, '') FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", [id], |r| r.get(0))?;
        let order = crate::venue::OrderRef::load(c, tx, &ext)?;
        // Every credential, proxy and account rule is the engine's: the venue is built as the engine builds it.
        let (venue, _) = crate::engine::run::venue_for(c, &factory(c, &inner), &inner.cipher, &bot)?;
        // The one cancel operation (with the adapter's own steps), bounded, never retried here. No transaction is open.
        let sent = runtime.block_on(tokio::time::timeout(crate::venue::http::TOTAL_TIMEOUT, venue.cancel_with(&order)));
        let (cancelled, invalid_key) = match &sent {
            Ok(Ok(())) => (true, false),
            Ok(Err(VenueError::Rejected(errs) | VenueError::Unauthorized(errs) | VenueError::Coded { errors: errs, .. })) => (false, venue.rules().is_invalid_key(errs)),
            // D-CANCEL-TRANSPORT-FAILURE: Rails' controller does not rescue a network failure (a 500); here it is a failure.
            Ok(Err(VenueError::Transient(_) | VenueError::Ambiguous(_))) | Err(_) => (false, false),
        };
        // D-CANCEL-REREAD-ALWAYS: whatever the answer, the row is re-read (a refusal may hide a cancel that landed); Rails
        // re-reads after a success only. The re-read is the engine's; this is the request's only write.
        crate::app_config::set_plain(c, &format!("{REREAD}{tx}"), &serde_json::json!({ "bot": id }).to_string(), inner.now()).map_err(WebError::Config)?;
        inner.wake_engine();
        Ok(Found::Answered { cancelled, invalid_key: invalid_key.then_some(name) })
    }).await.map_err(|e| WebError::Task(e.to_string()))?;
    match found {
        Ok(Found::NoBot) => Ok(page::not_found(&ctx)),
        Ok(Found::NoRow) => Ok(layout::missing()),
        Ok(Found::Refused) => answer(&ctx, false, None),
        Ok(Found::Answered { cancelled, invalid_key }) => answer(&ctx, cancelled, invalid_key.as_deref()),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// Rails' `render turbo_stream: turbo_stream_prepend_flash` with flash.now: notice on success, else alert (422).
fn answer(ctx: &Ctx, cancelled: bool, invalid_key: Option<&str>) -> Result<Response, WebError> {
    let (style, text) = match (cancelled, invalid_key) {
        (true, _) => ("primary", i18n::text(ctx.locale, "bot.messages.order_cancelled", &[])),
        (false, Some(exchange)) => ("danger", i18n::text(ctx.locale, "bot.messages.failed_to_cancel_order_invalid_api_key", &[("exchange_name", i18n::Arg::Text(exchange))])),
        (false, None) => ("danger", i18n::text(ctx.locale, "bot.messages.failed_to_cancel_order", &[])),
    };
    let body = turbo::prepend_flash(flash::render(&[flash::Message { style, text }])?.trim_end());
    Ok((if cancelled { StatusCode::OK } else { StatusCode::UNPROCESSABLE_ENTITY }, [(axum::http::header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response())
}
