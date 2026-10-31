//! The wash-sale rule on the bot pages (WashSalePromptable): the one-time selling warning, its modal and its acknowledgement.
//! The question itself (an account that has not answered) is still refused by `super::refusal` (PLAN-wash-sale WS-4b).
use super::page::{self, Found};
use super::Bot;
use crate::web::{auth, layout::{self, Ctx, Page}, shell::{self, Shell}, turbo, App, WebError};
use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};

#[derive(Template)]
#[template(path = "bots/wash_sale_prompts/selling_warning.html")]
struct SellingWarning<'a> { v: &'a Ctx, csrf: &'a str, path: String }

const EMPTY_FRAME: &str = "<turbo-frame id=\"modal\"></turbo-frame>";

/// WashSalePromptable#wash_sale_selling_warning_due?: the rule on, never warned, and a basket (not an index bot) whose
/// SAVED row sells or is armed to start selling.
pub fn warning_due(c: &Connection, user: i64, bot: i64) -> Result<bool, WebError> {
    let (enabled, warned): (Option<bool>, Option<String>) = c.query_row("SELECT wash_sale_enabled, wash_sale_selling_warned_at FROM users WHERE id = ?1", [user], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if enabled != Some(true) || warned.is_some() { return Ok(false); }
    let row: Option<(Option<String>, String)> = c.query_row("SELECT type, settings FROM bots WHERE id = ?1 AND user_id = ?2", [bot, user], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((Some(class), settings)) = row else { return Ok(false) };
    Ok(class == "Bots::DcaMultiAsset" && super::selling_or_armed(&serde_json::from_str(&settings).unwrap_or(serde_json::Value::Null)))
}

/// bots/wash_sale_prompts/_selling_warning.
fn warning(ctx: &Ctx, csrf: &str, bot: i64) -> Result<String, WebError> {
    Ok(SellingWarning { v: ctx, csrf, path: ctx.path(&format!("/bots/{bot}")) }.render()?)
}

/// WashSalePromptable#wash_sale_prompt_stream: the modal due, as a stream that replaces the layout's `modal` frame.
pub fn prompt_stream(c: &Connection, ctx: &Ctx, csrf: &str, user: i64, bot: i64) -> Result<Option<String>, WebError> {
    Ok(if warning_due(c, user, bot)? { Some(turbo::stream("replace", "modal", &warning(ctx, csrf, bot)?)) } else { None })
}

/// `content_for :modal_src` on the bot page: the modal is loaded from here when one is due.
pub fn modal_src(c: &Connection, ctx: &Ctx, user: i64, bot: &Bot) -> Result<Option<String>, WebError> {
    Ok(warning_due(c, user, bot.id)?.then(|| ctx.path(&format!("/bots/{}/wash_sale_prompt/new", bot.id))))
}

/// GET /bots/:id/wash_sale_prompt/new (Bots::WashSalePromptsController#new): the warning when due, else an empty frame,
/// so a stale page cannot pop a modal that is no longer owed.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let result = app.db(move |c| {
        let (found, _) = page::find(c, &inner, &owner, &segment, true, super::For::Settings, view.locale)?;
        let Found::Bot(bot) = found else { return Ok(Err(found)) };
        let csrf = view.csrf_token();
        let body = if warning_due(c, owner.id, bot.id)? { warning(&view, &csrf, bot.id)? } else { EMPTY_FRAME.to_string() };
        Ok(Ok((body, csrf, Shell::load(c, &inner, &owner)?)))
    }).await;
    match result {
        Ok(Ok((body, _, _))) if body == EMPTY_FRAME => Ok(([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()),
        Ok(Ok((body, csrf, shell))) => shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] }),
        Ok(Err(Found::NotPorted(reason))) => Ok(layout::refused(&ctx, reason)),
        // `current_user.bots.find` raises: Rails' 404.
        Ok(Err(_)) => Ok(layout::missing()),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// POST /bots/:id/wash_sale_selling_warning (Bots::WashSaleSellingWarningsController#create): "Got it" records the first
/// acknowledgement; later ones change nothing. The bot only has to be the user's (deleted ones too, as `bots.find`).
pub async fn acknowledge(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(layout::missing()) };
    let now = crate::codec::format_time(app.now());
    let owned = app.db(move |c| {
        let owned: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id = ?1 AND user_id = ?2)", [id, user], |r| r.get(0))?;
        if owned { c.execute("UPDATE users SET wash_sale_selling_warned_at = ?1, updated_at = ?1 WHERE id = ?2 AND wash_sale_selling_warned_at IS NULL", (&now, user))?; }
        Ok(owned)
    }).await?;
    Ok(if owned { StatusCode::NO_CONTENT.into_response() } else { layout::missing() })
}
