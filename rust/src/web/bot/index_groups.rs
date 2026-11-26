//! PLAN index-as-ingredient slice 5: the index sheet of a grouped basket and its routes. A refusal answers inside the sheet
//! (422, `update modal_content`), a saved slider redraws the sheet with the page, an operation that changes the content
//! (Change the index, Custom allocation, Remove, Add index) visits the bot.
use super::{action_params::ActionParams, action_view, index_switch::{self, Switched}, page, settings, write::{self, Outcome}, Bot, For};
use crate::web::{auth, layout::{self, Ctx, Page}, shell::{self, Shell}, turbo, wizard, App, WebError};
use axum::{extract::{Extension, Path, State}, http::StatusCode, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension};

/// The grouped basket `id` of `owner`, or None: not theirs, deleted (as the bot page refuses it), archived (whose page offers
/// no index item), a bot these pages do not render, or not stored with groups.
fn grouped(c: &Connection, owner: i64, id: i64, locale: &str) -> Result<Option<Bot>, WebError> {
    use crate::enums::BotStatus;
    let status: Option<i64> = c.query_row("SELECT status FROM bots WHERE id = ?1 AND user_id = ?2", [id, owner], |r| r.get(0)).optional()?;
    if status.is_none_or(|s| s == BotStatus::Deleted as i64 || s == BotStatus::Archived as i64) { return Ok(None); }
    Ok(Bot::find(c, owner, id, For::Settings, locale)?.filter(|bot| bot.grouped() && bot.unrendered().is_none()))
}

/// The sheet's frame content (the stored row), with `error` above the sliders.
fn sheet_frame(c: &Connection, ctx: &Ctx, owner: i64, id: i64, external_id: &str, error: Option<String>) -> Result<Option<String>, WebError> {
    let Some(bot) = grouped(c, owner, id, ctx.locale)? else { return Ok(None) };
    let Some(content) = settings::group_sheet(c, ctx, &ctx.csrf_token(), &bot, external_id, error)? else { return Ok(None) };
    Ok(Some(wizard::sheet_frame("", &content)?))
}

fn in_sheet(frame: &str) -> String { format!("{}\n", turbo::stream("update", "modal_content", &format!("{frame}\n"))) }

/// A full page holding `content` in the sheet over the bot's page (D-SHEET-NO-DIALOG), as `actions::index_new` serves it.
async fn sheet_page(app: App, ctx: Ctx, segment: String, content: impl FnOnce(&Connection, &Ctx, &Bot, &str) -> Result<Option<String>, WebError> + Send + 'static) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let (inner, mut view, owner) = (app.clone(), ctx.clone(), user.clone());
    let result = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        view.now = inner.now();
        let Some(bot) = grouped(&tx, owner.id, id, view.locale)? else { return Ok(None) };
        let csrf = view.csrf_token();
        let Some(content) = content(&tx, &view, &bot, &csrf)? else { return Ok(None) };
        let body = wizard::sheet_to(&view, &format!("/bots/{id}"), "", &content)?;
        let shell = Shell::load(&tx, &inner, &owner)?;
        tx.commit()?;
        Ok(Some((body, csrf, shell)))
    }).await;
    match result {
        Ok(Some((body, csrf, shell))) => shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] }),
        Ok(None) => Ok(page::not_found(&ctx)),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// GET /bots/:id/index_groups/:external_id/edit: the group's sheet.
pub async fn edit(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((segment, external_id)): Path<(String, String)>) -> Result<Response, WebError> {
    sheet_page(app, ctx, segment, move |c, ctx, bot, csrf| settings::group_sheet(c, ctx, csrf, bot, &external_id, None)).await
}

/// GET /bots/:id/index_groups/new: Add index, every held index left out.
pub async fn new(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    sheet_page(app, ctx, segment, |c, ctx, bot, csrf| Ok(Some(index_switch::group_picker(c, ctx, bot, csrf, None, None)?))).await
}

/// GET /bots/:id/index_groups/:external_id/index/new: Change the index of one group.
pub async fn change(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((segment, external_id)): Path<(String, String)>) -> Result<Response, WebError> {
    sheet_page(app, ctx, segment, move |c, ctx, bot, csrf| {
        if !bot.group_ids().contains(&external_id) { return Ok(None); }
        Ok(Some(index_switch::group_picker(c, ctx, bot, csrf, Some(&external_id), None)?))
    }).await
}

/// An operation's answer: a visit to the bot when done (the content changed), a refusal inside the sheet (`refused`).
fn answered(ctx: &Ctx, id: i64, switched: Switched, refused: impl FnOnce(String) -> Result<Option<String>, WebError>) -> Result<Response, WebError> {
    Ok(match switched {
        Switched::Done => action_view::response(turbo::redirect(&ctx.path(&format!("/bots/{id}"))), false),
        Switched::Refused(text) => match refused(text)? { Some(frame) => action_view::response(in_sheet(&frame), true), None => page::not_found(ctx) },
        Switched::Unported(reason) => layout::refused(ctx, reason),
        Switched::Missing | Switched::NotIndex => page::not_found(ctx),
    })
}

/// POST /bots/:id/index_groups: Add index (`index_category_id`), at 0 % beside the other ingredients.
pub async fn create(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let category = ctx.params.form("index_category_id").unwrap_or_default().to_string();
    let (inner, view) = (app.clone(), ctx.clone());
    let result = app.db(move |c| {
        if grouped(c, user.id, id, view.locale)?.is_none() { return Ok(Err(())); }
        let switched = index_switch::grouped_ops(c, &view, user.id, id, &[("add_index", &category)])?;
        if matches!(switched, Switched::Done) { inner.wake_engine(); inner.figure_service.mark(id); }
        let response = answered(&view, id, switched, |text| {
            let Some(bot) = grouped(c, user.id, id, view.locale)? else { return Ok(None) };
            Ok(Some(wizard::sheet_frame("", &index_switch::group_picker(c, &view, &bot, &view.csrf_token(), None, Some(text))?)?))
        })?;
        Ok(Ok(response))
    }).await;
    match result {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(())) => Ok(page::not_found(&ctx)),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// POST /bots/:id/index_groups/:external_id: the sheet's operations on that group: a picked index (`index_category_id`:
/// Change the index), `custom_allocation` (its members become plain assets) or `remove`.
pub async fn operate(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((segment, external_id)): Path<(String, String)>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let form = |key: &str| ctx.params.form(key).map(str::to_string);
    let fields: Vec<(&'static str, String)> = match (form("index_category_id"), form("custom_allocation"), form("remove")) {
        (Some(to), _, _) => vec![("change_index_id", external_id.clone()), ("change_index_to", to)],
        (None, Some(_), _) => vec![("ungroup_index_id", external_id.clone())],
        (None, None, Some(_)) => vec![("remove_index_id", external_id.clone())],
        _ => return Ok(StatusCode::BAD_REQUEST.into_response()),
    };
    let (inner, view) = (app.clone(), ctx.clone());
    let result = app.db(move |c| {
        if !grouped(c, user.id, id, view.locale)?.is_some_and(|bot| bot.group_ids().contains(&external_id)) { return Ok(Err(())); }
        let posted: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let switched = index_switch::grouped_ops(c, &view, user.id, id, &posted)?;
        if matches!(switched, Switched::Done) { inner.wake_engine(); inner.figure_service.mark(id); }
        Ok(Ok(answered(&view, id, switched, |text| sheet_frame(c, &view, user.id, id, &external_id, Some(text)))?))
    }).await;
    match result {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(())) => Ok(page::not_found(&ctx)),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// PATCH /bots/:id/index_groups/:external_id: the sheet's sliders through the one settings writer. Saved: the page's
/// streams and the sheet redrawn from the saved row (its drawn count and ceiling follow, so 3 → 4 → 3 saves 3). Refused:
/// 422, the sheet from the stored row with the message, nothing written.
pub async fn update(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path((segment, external_id)): Path<(String, String)>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let params = match ActionParams::parse(&ctx.params) { Ok(p) => p, Err(e) => return Ok(e.status().into_response()) };
    let scoped = only_this_group(params.value(), &external_id);
    let view = ctx.clone();
    let result = app.db(move |c| {
        if !grouped(c, user.id, id, view.locale)?.is_some_and(|bot| bot.group_ids().contains(&external_id)) { return Ok(Outcome::Missing); }
        // The sheet edits its own group's count and flattening, nothing else (Codex slice 5 branch r1): any other field, or
        // another group's, is refused whole, in the sheet.
        if !scoped {
            let message = crate::web::i18n::text(view.locale, "errors.bots.index_switch.not_offered", &[]);
            let frame = sheet_frame(c, &view, user.id, id, &external_id, Some(message))?.unwrap_or_default();
            return Ok(Outcome::Invalid(action_view::response(in_sheet(&frame), true), write::Failure::invalid("not_offered", String::new())));
        }
        write::settings(c, &view, user.id, id, &params, |c, ctx, draft| {
            let body = if draft.errors.is_empty() {
                let sheet = settings::group_sheet(c, ctx, &ctx.csrf_token(), &draft.candidate, &external_id, None)?.map(|content| wizard::sheet_frame("", &content)).transpose()?;
                action_view::settings_body(c, ctx, draft)? + &sheet.map(|frame| in_sheet(&frame)).unwrap_or_default()
            } else {
                let message = crate::v2::to_sentence(&draft.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>());
                let sheet = settings::group_sheet(c, ctx, &ctx.csrf_token(), &draft.original, &external_id, Some(message))?.map(|content| wizard::sheet_frame("", &content)).transpose()?;
                sheet.map(|frame| in_sheet(&frame)).unwrap_or_default()
            };
            Ok(write::Prepared { response: action_view::response(body, !draft.errors.is_empty()), broadcasts: vec![], after_scheduling: None })
        })
    }).await;
    match result {
        Ok(Outcome::Missing) => Ok(page::not_found(&ctx)),
        Ok(Outcome::Unported(reason)) => Ok(layout::refused(&ctx, reason)),
        Ok(Outcome::Committed(response) | Outcome::NoChange(response) | Outcome::Invalid(response, _) | Outcome::GuardRefused(response)) => Ok(response),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// The sheet's PATCH names only `bots_dca_multi_asset[index_groups][<external_id>]`'s count and flattening fields.
fn only_this_group(params: &serde_json::Value, external_id: &str) -> bool {
    const FIELDS: [&str; 4] = ["num_coins", "num_coins_rendered", "num_coins_ceiling", "allocation_flattening"];
    let Some(top) = params.as_object() else { return false };
    if top.keys().any(|k| !matches!(k.as_str(), "bots_dca_multi_asset" | "authenticity_token" | "_method")) { return false; }
    let Some(root) = top.get("bots_dca_multi_asset").and_then(serde_json::Value::as_object) else { return false };
    let Some(groups) = root.get("index_groups").and_then(serde_json::Value::as_object) else { return false };
    root.len() == 1 && groups.len() == 1 && groups.get(external_id).and_then(serde_json::Value::as_object).is_some_and(|f| !f.is_empty() && f.keys().all(|k| FIELDS.contains(&k.as_str())))
}
