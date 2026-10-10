//! Account settings first; later tasks add the remaining handlers.
pub mod account;pub mod confirmation;pub mod keys;pub mod mail;mod qr;pub mod two_fa;pub mod view;
use super::{auth,layout::{self,Ctx},App,WebError};
use axum::{extract::{Extension,State},http::{HeaderMap,Method,StatusCode},response::Response};
pub async fn root(Extension(ctx):Extension<Ctx>)->Response{layout::redirect(StatusCode::FOUND,&ctx.path("/settings/connect"))}
pub async fn show(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
 if ctx.user().is_none(){return Ok(auth::unauthenticated(&ctx))}
 match ctx.params.route_path.as_str(){"/settings/account"=>view::account(&app,&ctx,StatusCode::OK,None,vec![]).await,"/settings/edit_two_fa"=>two_fa::handle(app,ctx,false).await,_=>Ok(layout::not_ported_response(&ctx.method,&ctx.params.fullpath,ctx.turbo_frame.as_deref()))}
}
pub async fn write(State(app):State<App>,Extension(ctx):Extension<Ctx>,headers:HeaderMap)->Result<Response,WebError>{
 if ctx.user().is_none(){return Ok(auth::unauthenticated(&ctx))}
 if ctx.method==Method::PATCH&&ctx.params.route_path=="/settings/update_two_fa"{return two_fa::handle(app,ctx,true).await}
 if ctx.method==Method::PATCH&&["update_name","update_email","update_password","update_time_zone","update_locale"].iter().any(|name|ctx.params.route_path==format!("/settings/{name}")){return account::write(app,ctx,headers).await}
 Ok(layout::not_ported_response(&ctx.method,&ctx.params.fullpath,ctx.turbo_frame.as_deref()))
}
