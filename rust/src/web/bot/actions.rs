//! Existing-bot HTTP actions. Authentication, CSRF, bounds and format checks stay in pipeline.
use super::{action_params::ActionParams, action_view, page, write::{self, Action, Outcome}};
use crate::web::{auth, flash, i18n, layout::{self,Ctx,Page}, shell::{self,Shell}, App, WebError};
use axum::{extract::{Extension,Path,State}, http::StatusCode, response::{IntoResponse,Response}};

async fn mutate(app: App, ctx: Ctx, segment: String, action: Option<Action>) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let segment = segment.strip_suffix(".turbo_stream").unwrap_or(&segment);
    let Some(id) = super::id_from_path(segment) else { return Ok(page::not_found(&ctx)) };
    let params = match ActionParams::parse(&ctx.params) { Ok(p)=>p,Err(e)=>return Ok(e.status().into_response()) };
    let view = ctx.clone();
    let result = app.db(move |c| {
        if let Some(action) = action {
            write::lifecycle(c,&view,user.id,id,action,&params,|c,ctx,state| {
                let label: Option<String> = c.query_row("SELECT label FROM bots WHERE id=?1 AND user_id=?2",(id,user.id),|r|r.get(0))?;
                let prepared = action_view::lifecycle_response(c,ctx,id,action,state)?;
                Ok(write::Prepared { response: (prepared.response,label), broadcasts: prepared.broadcasts })
            })
        } else {
            write::settings(c,&view,user.id,id,&params,|c,ctx,draft| {
                let mut prepared = action_view::settings_response(c,ctx,draft)?;
                if draft.errors.iter().any(|error|error.field == "settings" && error.message == "missing or invalid bot parameter root") {
                    *prepared.response.status_mut() = StatusCode::BAD_REQUEST;
                }
                Ok(write::Prepared { response: (prepared.response,None), broadcasts: prepared.broadcasts })
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
        Ok(Outcome::NoChange((response,_)) | Outcome::Invalid((response,_)) | Outcome::GuardRefused((response,_))) => Ok(response),
        Err(e) => layout::or_refused(&ctx,e),
    }
}
macro_rules! mutation {
    ($name:ident, $action:expr) => {
        pub async fn $name(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(id): Path<String>) -> Result<Response,WebError> {
            mutate(app,ctx,id,$action).await
        }
    };
}
mutation!(update,None);
mutation!(start,Some(Action::Start));
mutation!(stop,Some(Action::Stop));
mutation!(delete,Some(Action::Delete));
mutation!(archive,Some(Action::Archive));
mutation!(unarchive,Some(Action::Unarchive));

async fn form(app: App, ctx: Ctx, segment: String, form: &'static str) -> Result<Response,WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner,mut view,owner) = (app.clone(),ctx.clone(),user.clone());
    let result = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        view.now = inner.now();
        let (found,_) = page::find(&tx,&inner,&owner,&segment,false,super::For::Page)?;
        let page::Found::Bot(bot) = found else { return Ok(Err(found)) };
        let csrf = if form == "start" && !bot.restarting() { String::new() } else { view.csrf_token() };
        let body = action_view::modal(&tx,&view,&bot,form,&csrf)?;
        let shell = Shell::load(&tx,&inner,&owner)?;
        tx.commit()?;
        Ok(Ok((body,csrf,shell)))
    }).await;
    match result {
        Ok(Ok((body,_,_))) if body == "<turbo-frame id=\"modal\"></turbo-frame>" => Ok(([(axum::http::header::CONTENT_TYPE,"text/html; charset=utf-8")],body).into_response()),
        Ok(Ok((body,csrf,shell))) => shell::application(&ctx,&csrf,&user,&shell,Page { status:StatusCode::OK,body,flash_now:vec![] }),
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
