//! Existing-bot HTTP actions. Authentication, CSRF, bounds and format checks stay in pipeline.
use super::{action_params::ActionParams, action_view, page, write::{self, Action, Outcome}};
use crate::web::{auth, flash, i18n, layout::{self,Ctx,Page}, shell::{self,Shell}, App, WebError};
use axum::{extract::{Extension,Path,State}, http::{HeaderMap,StatusCode}, response::{IntoResponse,Response}};

async fn mutate(app: App, ctx: Ctx, segment: String, action: Option<Action>, headers: HeaderMap) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let segment = segment.strip_suffix(".turbo_stream").unwrap_or(&segment);
    let Some(id) = super::id_from_path(segment) else { return Ok(page::not_found(&ctx)) };
    let params = match ActionParams::parse(&ctx.params) { Ok(p)=>p,Err(e)=>return Ok(e.status().into_response()) };
    let owner = user.id;
    // Bots::StartsController#update asks the wash-sale question before anything else, of every kind of bot it starts.
    if action == Some(Action::Start) {
        let view = ctx.clone();
        if let Some(refused) = app.db(move |c| super::wash::owed_start(c, &view, owner, id)).await? { return Ok(refused); }
    }
    // A signal bot has its own thin model (bot::signal_write).
    if app.db(move |c| super::signal_write::is_signal(c, owner, id)).await? {
        return super::signal_write::web_action(app, ctx, owner, id, action, params, &headers).await;
    }
    let view = ctx.clone();
    let result = app.db(move |c| {
        if let Some(action) = action {
            write::lifecycle(c,&view,user.id,id,action,&params,|c,ctx,state| {
                let (label,class,settings): (Option<String>,Option<String>,String) = c.query_row("SELECT label,type,settings FROM bots WHERE id=?1 AND user_id=?2",(id,user.id),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                // Rails flashes the deleted bot's loaded label: one stored without a name is named for it.
                let label = match label.filter(|label| !label.trim().is_empty()) {
                    Some(label) => Some(label),
                    None if action != Action::Delete => None,
                    None => Some(crate::web::mcp::reads::generated_label(c,user.id,id,class.as_deref().unwrap_or_default(),&serde_json::from_str(&settings).unwrap_or(serde_json::Value::Null),ctx.locale)?),
                };
                let prepared = action_view::lifecycle_response(c,ctx,id,action,state)?;
                Ok(write::Prepared { response: (prepared.response,label), broadcasts: prepared.broadcasts, after_scheduling: prepared.after_scheduling })
            })
        } else {
            write::settings(c,&view,user.id,id,&params,|c,ctx,draft| {
                let mut prepared = action_view::settings_response(c,ctx,draft)?;
                if draft.errors.iter().any(|error|error.field == "settings" && error.message == "missing or invalid bot parameter root") {
                    *prepared.response.status_mut() = StatusCode::BAD_REQUEST;
                }
                Ok(write::Prepared { response: (prepared.response,None), broadcasts: prepared.broadcasts, after_scheduling: prepared.after_scheduling })
            })
        }
    }).await;
    match result {
        Ok(Outcome::Missing) => Ok(page::not_found(&ctx)),
        Ok(Outcome::Unported(reason)) => Ok(layout::refused(&ctx,reason)),
        Ok(Outcome::Committed((response,label))) => {
            if action == Some(Action::Delete) {
                flash::set(&ctx.session,flash::NOTICE,i18n::text(ctx.locale,"errors.bots.destroy_success",&[("bot_label",i18n::Arg::Text(label.as_deref().unwrap_or("")))]));
            }
            Ok(response)
        }
        Ok(Outcome::NoChange((response,_)) | Outcome::Invalid((response,_),_) | Outcome::GuardRefused((response,_))) => Ok(response),
        Err(e) => layout::or_refused(&ctx,e),
    }
}
macro_rules! mutation {
    ($name:ident, $action:expr) => {
        pub async fn $name(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response,WebError> {
            mutate(app,ctx,id,$action,headers).await
        }
    };
}
mutation!(update,None);
mutation!(start,Some(Action::Start));
mutation!(stop,Some(Action::Stop));
mutation!(delete,Some(Action::Delete));
mutation!(archive,Some(Action::Archive));
mutation!(unarchive,Some(Action::Unarchive));

/// POST /bots/:id/reverse (BotsController#reverse). The rotation commits in one transaction (write::reverse) and the engine
/// is woken to cancel what it captured. The page then refreshes, where Rails replaces its panels: the metrics panel it
/// re-renders is drawn from figures a request here does not compute (a divergence listed in tests/pages.rs).
pub async fn reverse(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let segment = segment.strip_suffix(".turbo_stream").unwrap_or(&segment);
    let Some(id) = super::id_from_path(segment) else { return Ok(page::not_found(&ctx)) };
    let inner = app.clone();
    let result = app.db(move |c| {
        let reversed = write::reverse(c, &inner.cipher, user.id, id, inner.now())?;
        // In the App::db job, after the commit, as every writer wakes the engine.
        if matches!(reversed, write::Reversal::Reversed) { inner.wake_engine(); }
        Ok(reversed)
    }).await;
    let flash = |style: &'static str, text: String| -> Result<String,WebError> { Ok(crate::web::turbo::prepend_flash(flash::render(&[flash::Message { style, text }])?.trim_end())) };
    match result {
        Ok(write::Reversal::Missing) => Ok(page::not_found(&ctx)),
        Ok(write::Reversal::InProgress) => Ok(action_view::response(flash("primary", i18n::text(ctx.locale,"bot.reverse_in_progress",&[]))?,false)),
        Ok(write::Reversal::Refused(reason)) => Ok(action_view::response(flash("danger", i18n::text(ctx.locale,"engine.write_refused",&[("reason",i18n::Arg::Text(&reason))]))?,true)),
        Ok(write::Reversal::Reversed) => Ok(action_view::response(crate::web::turbo::refresh().into(),false)),
        Err(e) => layout::or_refused(&ctx,e),
    }
}

async fn form(app: App, ctx: Ctx, segment: String, form: &'static str) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner,mut view,owner) = (app.clone(),ctx.clone(),user.clone());
    let result = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        view.now = inner.now();
        let (found,_) = page::find(&tx,&inner,&owner,&segment,false,super::For::Settings,view.locale)?;
        let page::Found::Bot(bot) = found else { return Ok(Err(found)) };
        // Bots::StartsController#edit: the wash-sale question in place of the restart question while it is owed.
        let dialog = if form == "start" { let csrf = view.csrf_token(); super::wash::start_dialog(&tx,&view,&csrf,owner.id,bot.id)?.map(|body| (body,csrf)) } else { None };
        if let Some((body,_)) = dialog { tx.commit()?; return Ok(Ok((body,String::new(),None))); }
        let csrf = if form == "start" && !bot.restarting() { String::new() } else { view.csrf_token() };
        let body = action_view::modal(&tx,&view,&bot,form,&csrf)?;
        let shell = Shell::load(&tx,&inner,&owner)?;
        tx.commit()?;
        Ok(Ok((body,csrf,Some(shell))))
    }).await;
    match result {
        // A bare partial (`render partial:` / `render html:`): no layout.
        Ok(Ok((body,_,shell))) if shell.is_none() || body == super::wash::EMPTY_FRAME => Ok(([(axum::http::header::CONTENT_TYPE,"text/html; charset=utf-8")],body).into_response()),
        Ok(Ok((body,csrf,Some(shell)))) => shell::application(&ctx,&csrf,&user,&shell,Page { status:StatusCode::OK,body,flash_now:vec![] }),
        Ok(Ok((_,_,None))) => Err(WebError::Config("unexpected form result".into())),
        Ok(Err(page::Found::Missing)) => Ok(page::not_found(&ctx)),
        Ok(Err(page::Found::NotPorted(reason))) => Ok(layout::refused(&ctx,reason)),
        Ok(Err(page::Found::Bot(_))) => Err(WebError::Config("unexpected form result".into())),
        Err(e) => layout::or_refused(&ctx,e),
    }
}
macro_rules! modal {
    ($name:ident, $form:literal) => {
        pub async fn $name(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(id): Path<String>) -> Result<Response,WebError> {
            form(app,ctx,id,$form).await
        }
    };
}
modal!(edit,"rename");
modal!(delete_edit,"delete");
modal!(archive_edit,"archive");
modal!(start_edit,"start");

/// POST /bots/:id/custom_allocation (Bots::CustomAllocationsController#create): one write transaction (index_switch::customize),
/// then Turbo visits the bot as a portfolio. A refusal is the flash Rails prepends, with 422.
pub async fn custom_allocation(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>, headers: HeaderMap) -> Result<Response,WebError> {
    use super::index_switch::{customize, Switched};
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let (inner, view) = (app.clone(), ctx.clone());
    let result = app.db(move |c| {
        let switched = customize(c, &view, user.id, id)?;
        // In the App::db job, after the commit, as every writer wakes the engine.
        if matches!(switched, Switched::Done) { inner.wake_engine(); inner.figure_service.mark(id); }
        Ok(switched)
    }).await;
    match result {
        Ok(switched) => switched_response(&ctx, &headers, id, switched),
        Err(e) => layout::or_refused(&ctx,e),
    }
}

/// `head :not_found`, in the request's format.
fn head_not_found(headers: &HeaderMap) -> Response {
    (StatusCode::NOT_FOUND, [(axum::http::header::CONTENT_TYPE, if crate::web::turbo::accepts_stream(headers) { "text/vnd.turbo-stream.html" } else { "text/html" })]).into_response()
}

/// Bot::IndexSwitch's answers: Turbo visits the switched bot; a refusal is the flash Rails prepends, with 422.
fn switched_response(ctx: &Ctx, headers: &HeaderMap, id: i64, switched: super::index_switch::Switched) -> Result<Response,WebError> {
    use super::index_switch::Switched;
    match switched {
        Switched::Missing => Ok(page::not_found(ctx)),
        Switched::NotIndex => Ok(head_not_found(headers)),
        Switched::Unported(reason) => Ok(layout::refused(ctx,reason)),
        Switched::Refused(text) => Ok(action_view::response(crate::web::turbo::prepend_flash(flash::render(&[flash::Message { style: "danger", text }])?.trim_end()),true)),
        Switched::Done => Ok(action_view::response(crate::web::turbo::redirect(&ctx.path(&format!("/bots/{id}"))),false)),
    }
}

/// GET /bots/:id/index/new (Bots::IndicesController#new): the bot menu's index picker, in the modal frame.
pub async fn index_new(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>, headers: HeaderMap) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let (inner,mut view,owner) = (app.clone(),ctx.clone(),user.clone());
    let result: Result<Result<_,bool>,WebError> = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        view.now = inner.now();
        match super::index_switch::composition_bot(&tx,&view,owner.id,id)? {
            None => return Ok(Err(false)),
            Some(false) => return Ok(Err(true)),
            Some(true) => {}
        }
        // The picker reads the bot's class, venue and quote only: it is served whatever the bot's history or state.
        let Some(bot) = super::Bot::find(&tx,owner.id,id,super::For::Feed,view.locale)? else { return Ok(Err(false)) };
        let csrf = view.csrf_token();
        let content = super::index_switch::picker(&tx,&view,&bot,&csrf)?;
        let body = askama::Template::render(&crate::web::wizard::keys::FullPage { close_url: view.path(&format!("/bots/{id}")), content: &content })?;
        let shell = Shell::load(&tx,&inner,&owner)?;
        tx.commit()?;
        Ok(Ok((body,csrf,shell)))
    }).await;
    match result {
        Ok(Ok((body,csrf,shell))) => shell::application(&ctx,&csrf,&user,&shell,Page { status:StatusCode::OK,body,flash_now:vec![] }),
        // Bots::Botable#set_bot's redirect; require_composition_bot's `head :not_found`.
        Ok(Err(false)) => Ok(page::not_found(&ctx)),
        Ok(Err(true)) => Ok(head_not_found(&headers)),
        Err(e) => layout::or_refused(&ctx,e),
    }
}

/// POST /bots/:id/index (Bots::IndicesController#create): the picked tile's index, followed in one write transaction
/// (index_switch::follow); the engine then derives the members.
pub async fn index_create(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>, headers: HeaderMap) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(page::not_found(&ctx)) };
    let category = ctx.params.form("index_category_id").or_else(|| ctx.params.query("index_category_id")).map(str::to_string);
    let (inner, view) = (app.clone(), ctx.clone());
    let result = app.db(move |c| {
        let switched = super::index_switch::follow(c, &view, user.id, id, category.as_deref())?;
        // In the App::db job, after the commit, as every writer wakes the engine (which owes the bot its resync).
        if matches!(switched, super::index_switch::Switched::Done) { inner.wake_engine(); inner.figure_service.mark(id); }
        Ok(switched)
    }).await;
    match result {
        Ok(switched) => switched_response(&ctx, &headers, id, switched),
        Err(e) => layout::or_refused(&ctx,e),
    }
}

/// GET /bots/merge/new (Bots::MergesController#new): the merge modal for the picked bots, refusal included; a venue picked in
/// its pill re-requests it into the merge_modal frame.
pub async fn merge_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Ok(params) = ActionParams::parse_rails(&ctx.params) else { return Ok((StatusCode::BAD_REQUEST, "Bad Request\n").into_response()) };
    let (ids, exchange) = super::merge::params(params.value());
    let (inner,mut view,owner) = (app.clone(),ctx.clone(),user.clone());
    let result = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        view.now = inner.now();
        let mut merge = match super::merge::Merge::load(&tx,&view,owner.id,ids,exchange)? {
            super::merge::Loaded::Merge(merge) => merge,
            super::merge::Loaded::Unported(reason) => return Ok(Err(reason)),
        };
        let csrf = view.csrf_token();
        let body = super::merge::modal(&tx,&view,&mut merge,&csrf)?;
        let shell = Shell::load(&tx,&inner,&owner)?;
        tx.commit()?;
        Ok(Ok((body,csrf,shell)))
    }).await;
    match result {
        Ok(Ok((body,csrf,shell))) => shell::application(&ctx,&csrf,&user,&shell,Page { status:StatusCode::OK,body,flash_now:vec![] }),
        Ok(Err(reason)) => Ok(layout::refused(&ctx,reason)),
        Err(e) => layout::or_refused(&ctx,e),
    }
}

/// POST /bots/merge (Bots::MergesController#create): one write transaction (merge::perform), then Turbo visits the merged bot;
/// a refusal is the flash Rails prepends, with 422.
pub async fn merge_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response,WebError> {
    use super::merge::{perform, Merged};
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Ok(params) = ActionParams::parse_rails(&ctx.params) else { return Ok((StatusCode::BAD_REQUEST, "Bad Request\n").into_response()) };
    let (ids, exchange) = super::merge::params(params.value());
    let (inner, view) = (app.clone(), ctx.clone());
    let result = app.db(move |c| {
        let merged = perform(c, &view, user.id, ids, exchange)?;
        // In the App::db job, after the commit, as every writer wakes the engine.
        if let Merged::Done(id) = merged { inner.wake_engine(); inner.figure_service.mark(id); inner.figure_service.mark_owner(user.id); }
        Ok(merged)
    }).await;
    match result {
        Ok(Merged::Done(id)) => {
            flash::set(&ctx.session, flash::NOTICE, i18n::text(ctx.locale, "bot.merge.success", &[]));
            Ok(action_view::response(crate::web::turbo::redirect(&ctx.path(&format!("/bots/{id}"))), false))
        }
        Ok(Merged::Refused(text)) => Ok(action_view::response(crate::web::turbo::prepend_flash(flash::render(&[flash::Message { style: "danger", text }])?.trim_end()), true)),
        Ok(Merged::Unported(reason)) => Ok(layout::refused(&ctx, reason)),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// GET /bots/split/new (Bots::SplitsController#new): the split modal for the picked bots, refusal included.
pub async fn split_new(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response,WebError> {
    use super::split::{modal, Loaded, Split};
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Ok(params) = ActionParams::parse_rails(&ctx.params) else { return Ok((StatusCode::BAD_REQUEST, "Bad Request\n").into_response()) };
    let (ids, keep) = super::split::params(params.value());
    let (inner,mut view,owner) = (app.clone(),ctx.clone(),user.clone());
    let result = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        view.now = inner.now();
        let mut split = match Split::load(&tx,&view,owner.id,ids,keep)? { Loaded::Split(split) => split, Loaded::Unported(reason) => return Ok(Err(reason)) };
        let csrf = view.csrf_token();
        let body = match modal(&tx,&view,&mut split,&csrf)? { Ok(body) => body, Err(reason) => return Ok(Err(reason)) };
        let shell = Shell::load(&tx,&inner,&owner)?;
        tx.commit()?;
        Ok(Ok((body,csrf,shell)))
    }).await;
    match result {
        Ok(Ok((body,csrf,shell))) => shell::application(&ctx,&csrf,&user,&shell,Page { status:StatusCode::OK,body,flash_now:vec![] }),
        Ok(Err(reason)) => Ok(layout::refused(&ctx,reason)),
        Err(e) => layout::or_refused(&ctx,e),
    }
}

/// POST /bots/split (Bots::SplitsController#create): one write transaction (split::perform), split::complete after it, then
/// Turbo visits the dashboard; a refusal is the flash Rails prepends, with 422, and so is a split this build does not serve
/// (`engine.write_refused`, naming why), so the modal that posted it says so in place.
pub async fn split_create(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response,WebError> {
    use super::split::{complete, perform, Performed};
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let Ok(params) = ActionParams::parse_rails(&ctx.params) else { return Ok((StatusCode::BAD_REQUEST, "Bad Request\n").into_response()) };
    let (ids, keep) = super::split::params(params.value());
    let (inner, view) = (app.clone(), ctx.clone());
    let result = app.db(move |c| {
        let performed = perform(c, &view, user.id, ids, keep)?;
        // In the App::db job, after the commit, as every writer wakes the engine.
        if let Performed::Done(done) = &performed { complete(&inner, user.id, done); }
        Ok(performed)
    }).await;
    match result {
        Ok(Performed::Done(_)) => {
            flash::set(&ctx.session, flash::NOTICE, i18n::text(ctx.locale, "bot.split.success", &[]));
            Ok(action_view::response(crate::web::turbo::redirect(&ctx.path("/bots")), false))
        }
        Ok(Performed::Refused(refusal)) => Ok(action_view::response(crate::web::turbo::prepend_flash(flash::render(&[flash::Message { style: "danger", text: refusal.reason }])?.trim_end()), true)),
        Ok(Performed::Unported(reason)) => {
            eprintln!("deltabadger: {} {} is not served: {reason}", ctx.method, ctx.params.fullpath);
            let text = i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(reason))]);
            Ok(action_view::response(crate::web::turbo::prepend_flash(flash::render(&[flash::Message { style: "danger", text }])?.trim_end()), true))
        }
        Err(e) => layout::or_refused(&ctx, e),
    }
}
