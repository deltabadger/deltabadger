//! The wash-sale rule on the bot pages (WashSalePromptable): the one-time question an account that has not answered is
//! asked at the first bot that can sell, and the one-time selling warning an account with the rule on is given.
use super::Kind;
use crate::web::{auth, flash, i18n, layout::{self, Ctx, Page}, shell::{self, Shell}, turbo, App, WebError};
use askama::Template;
use axum::extract::{Extension, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

#[derive(Template)]
#[template(path = "bots/wash_sale_prompts/selling_warning.html")]
struct SellingWarning<'a> { v: &'a Ctx, csrf: &'a str, path: String }

#[derive(Template)]
#[template(path = "bots/wash_sale_prompts/dialog.html")]
struct Dialog<'a> { v: &'a Ctx, csrf: &'a str, path: String, question: String }

pub const EMPTY_FRAME: &str = "<turbo-frame id=\"modal\"></turbo-frame>";

/// A bot row's class and settings, as saved.
fn saved(c: &Connection, user: i64, bot: i64) -> Result<Option<(Kind, Value)>, WebError> {
    let row: Option<(Option<String>, String)> = c.query_row("SELECT type, settings FROM bots WHERE id = ?1 AND user_id = ?2", [bot, user], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    Ok(row.map(|(class, settings)| (Kind::of(class.as_deref().unwrap_or_default()), serde_json::from_str(&settings).unwrap_or(Value::Null))))
}

/// Bot#sell_capable?, on the settings given: a basket that sells or is armed to (Bot::Reversible), a rebalance rule, or a
/// holding of at least the venue's minimum (Liquidatable#held_symbols, the walk without prices); a signal bot's enabled sell
/// rule (Bots::Signal#sell_capable?).
fn sell_capable(c: &Connection, bot: i64, kind: Kind, settings: &Value, now: DateTime<Utc>) -> Result<bool, WebError> {
    let class = match kind { Kind::Signal => return super::signal::sells(c, bot), Kind::Index => "Bots::DcaIndex", Kind::Basket => "Bots::DcaMultiAsset" };
    if super::sell_capable(class, settings) { return Ok(true); }
    // Only a filled order holds something: the walk is not run for a bot that has none.
    let filled: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND status = 0)", [bot], |r| r.get(0))?;
    Ok(filled && !super::liquidation::held(c, bot, now)?.is_empty())
}

/// The user's answer (nil, false or true), whether the selling warning was given, and the jurisdiction as stored.
fn account(c: &Connection, user: i64) -> Result<(Option<bool>, bool, Option<String>), WebError> {
    Ok(c.query_row("SELECT wash_sale_enabled, wash_sale_selling_warned_at IS NOT NULL, wash_sale_jurisdiction FROM users WHERE id = ?1", [user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?)
}

/// What is due over a bot, judged on the settings given: the question first (BotApi::Bots::Start.wash_sale_question_owed?),
/// else the selling warning (wash_sale_selling_warning_due?: the rule on, never warned, a basket that sells or is armed to).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Due { Question, Warning }

fn due_on(c: &Connection, user: i64, bot: i64, kind: Kind, settings: &Value, now: DateTime<Utc>) -> Result<Option<Due>, WebError> {
    let (enabled, warned, _) = account(c, user)?;
    Ok(match enabled {
        None if sell_capable(c, bot, kind, settings, now)? => Some(Due::Question),
        Some(true) if !warned && kind == Kind::Basket && super::selling_or_armed(settings) => Some(Due::Warning),
        _ => None,
    })
}

/// `due_on` the row as saved; nothing for a bot that is not the user's.
pub fn due(c: &Connection, user: i64, bot: i64, now: DateTime<Utc>) -> Result<Option<Due>, WebError> {
    let Some((kind, settings)) = saved(c, user, bot)? else { return Ok(None) };
    due_on(c, user, bot, kind, &settings, now)
}

/// The modal the `Due` names: bots/wash_sale_prompts/_dialog or _selling_warning.
fn modal(c: &Connection, ctx: &Ctx, csrf: &str, user: i64, bot: i64, due: Due) -> Result<String, WebError> {
    let path = ctx.path(&format!("/bots/{bot}"));
    Ok(match due {
        // The prompt picks neither answer (`checked` nil); the pill shows the account's jurisdiction.
        Due::Question => Dialog { v: ctx, csrf, path, question: crate::web::settings::view::question(ctx, None, account(c, user)?.2.as_deref()) }.render()?,
        Due::Warning => SellingWarning { v: ctx, csrf, path }.render()?,
    })
}

fn stream(c: &Connection, ctx: &Ctx, csrf: &str, user: i64, bot: i64, due: Option<Due>) -> Result<Option<String>, WebError> {
    due.map(|due| Ok(turbo::stream("replace", "modal", &modal(c, ctx, csrf, user, bot, due)?))).transpose()
}

/// WashSalePromptable#wash_sale_prompt_stream: the modal due, as a stream that replaces the layout's `modal` frame.
pub fn prompt_stream(c: &Connection, ctx: &Ctx, csrf: &str, user: i64, bot: i64) -> Result<Option<String>, WebError> {
    stream(c, ctx, csrf, user, bot, due(c, user, bot, ctx.now)?)
}

/// `prompt_stream` judged on the settings as saved (a settings save that failed: its tentative writes are not the row).
pub fn saved_prompt_stream(c: &Connection, ctx: &Ctx, csrf: &str, user: i64, bot: i64, kind: Kind, settings: &Value) -> Result<Option<String>, WebError> {
    stream(c, ctx, csrf, user, bot, due_on(c, user, bot, kind, settings, ctx.now)?)
}

/// `content_for :modal_src` on the bot page: the modal is loaded from here when one is due.
pub fn modal_src(c: &Connection, ctx: &Ctx, user: i64, bot: i64) -> Result<Option<String>, WebError> {
    Ok(due(c, user, bot, ctx.now)?.map(|_| ctx.path(&format!("/bots/{bot}/wash_sale_prompt/new"))))
}

/// Bots::StartsController#edit while the question is owed: the dialog in place of the restart question, as a bare partial.
pub fn start_dialog(c: &Connection, ctx: &Ctx, csrf: &str, user: i64, bot: i64) -> Result<Option<String>, WebError> {
    if due(c, user, bot, ctx.now)? != Some(Due::Question) { return Ok(None); }
    modal(c, ctx, csrf, user, bot, Due::Question).map(Some)
}

/// Bots::StartsController#update while the question is owed: refused with the question itself, 422
/// (`[wash_sale_prompt_stream, flash engine.wash_sale_question_owed]`). Nothing for a bot that is not the user's or not
/// owed it: the start answers as it does.
pub fn owed_start(c: &Connection, ctx: &Ctx, user: i64, bot: i64) -> Result<Option<Response>, WebError> {
    if due(c, user, bot, ctx.now)? != Some(Due::Question) { return Ok(None); }
    let Some(found) = super::Bot::find(c, user, bot, super::For::Settings, ctx.locale)? else { return Ok(None) };
    let message = i18n::text(ctx.locale, "engine.wash_sale_question_owed", &[("label", i18n::Arg::Text(&found.label))]);
    let csrf = ctx.csrf_token();
    let dialog = turbo::stream("replace", "modal", &modal(c, ctx, &csrf, user, bot, Due::Question)?);
    let flash = turbo::prepend_flash(flash::render(&flash::take(&ctx.session, &[(flash::ALERT, message)]))?.trim_end());
    Ok(Some((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], dialog + &flash).into_response()))
}

/// `current_user.bots.find` (deleted bots too) is all Bots::WashSalePrompts asks: none of the page's refusals, so a page
/// loaded before a bot changed still gets its modal.
fn owned(c: &Connection, user: i64, bot: i64) -> Result<bool, WebError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM bots WHERE id = ?1 AND user_id = ?2)", [bot, user], |r| r.get(0))?)
}

/// GET /bots/:id/wash_sale_prompt/new (Bots::WashSalePromptsController#new): the question when due, else the warning, else an
/// empty frame, so a stale page cannot pop a modal that is no longer owed.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(layout::missing()) };
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let result = app.db(move |c| {
        if !owned(c, owner.id, id)? { return Ok(None); }
        let csrf = view.csrf_token();
        let body = match due(c, owner.id, id, view.now)? { Some(due) => modal(c, &view, &csrf, owner.id, id, due)?, None => EMPTY_FRAME.to_string() };
        Ok(Some((body, csrf, Shell::load(c, &inner, &owner)?)))
    }).await;
    match result {
        Ok(Some((body, csrf, shell))) => shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] }),
        Ok(None) => Ok(layout::missing()),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// POST /bots/:id/wash_sale_prompt (Bots::WashSalePromptsController#create): the answer, through the one writer the account
/// box uses. Neither choice picked, or an unknown jurisdiction, is a 422 that writes nothing; else the flash, and the modal
/// closes itself (modal--base#submitEnd).
pub async fn answer(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    use crate::web::settings::tax::{self, Decision};
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(layout::missing()) };
    if !app.db(move |c| owned(c, user, id)).await? { return Ok(layout::missing()); }
    let Some(answer) = ctx.params.form("wash_sale[enabled]").filter(|s| !crate::ruby::blank(s)) else {
        return tax::refuse(&ctx, i18n::text(ctx.locale, "settings.wash_sale.prompt_missing", &[]));
    };
    let enabled = !["0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&answer);
    let jurisdiction = ctx.params.form("wash_sale[jurisdiction]").filter(|s| !crate::ruby::blank(s)).map(str::to_string);
    let (jobs, inner) = (app.job_wakers()?, app.clone());
    match app.db(move |c| tax::record_decision(c, &inner, &jobs, user, enabled, jurisdiction)).await? {
        // `errors.full_messages.to_sentence`: the attribute's name with the inclusion message.
        Decision::Invalid => tax::refuse(&ctx, crate::web::setup::user_full_message(ctx.locale, "wash_sale_jurisdiction", "inclusion")),
        Decision::Refused(reason) => tax::refuse(&ctx, i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&reason))])),
        Decision::Saved => {
            let markup = flash::render(&flash::take(&ctx.session, &[]))?;
            Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], turbo::prepend_flash(markup.trim_end())).into_response())
        }
    }
}

/// POST /bots/:id/wash_sale_selling_warning (Bots::WashSaleSellingWarningsController#create): "Got it" records the first
/// acknowledgement; later ones change nothing. The bot only has to be the user's (deleted ones too, as `bots.find`).
pub async fn acknowledge(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|user| user.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(layout::missing()) };
    let now = crate::codec::format_time(app.now());
    let owned = app.db(move |c| {
        let owned = owned(c, user, id)?;
        if owned { c.execute("UPDATE users SET wash_sale_selling_warned_at = ?1, updated_at = ?1 WHERE id = ?2 AND wash_sale_selling_warned_at IS NULL", (&now, user))?; }
        Ok(owned)
    }).await?;
    Ok(if owned { StatusCode::NO_CONTENT.into_response() } else { layout::missing() })
}
