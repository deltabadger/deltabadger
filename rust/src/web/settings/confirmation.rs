//! J: confirmation has Rails' unlimited lifetime. The current token authorizes one pending change.
use crate::{codec::format_time, web::{auth, flash, i18n, layout::{self, Ctx, Page}, App, WebError}};
use askama::Template;
use axum::{extract::{Extension, State}, http::StatusCode, response::Response};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use subtle::ConstantTimeEq;
#[derive(Template)]
#[template(path="settings/confirmation.html")]
struct Form<'a>{v:&'a Ctx,csrf:&'a str,new:bool,persisted:bool,error:Option<String>,registration_open:bool}
async fn form(app:&App,ctx:&Ctx,new:bool,persisted:bool,error:Option<String>)->Result<Response,WebError>{
    let inner=app.clone();
    let registration_open=app.db(move|c|Ok(auth::app_config(c,&inner.cipher,"registration_open")?.as_deref()==Some("true"))).await?;
    let csrf=ctx.csrf_token();
    let body=Form{v:ctx,csrf:&csrf,new,persisted,error,registration_open}.render()?;
    layout::devise(ctx,&csrf,Page{status:StatusCode::OK,body,flash_now:vec![]})
}
pub async fn new(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{form(&app,&ctx,true,false,None).await}
pub async fn show(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
    let token=ctx.params.query("confirmation_token").unwrap_or("").to_string();
    let inner=app.clone();
    let (success,persisted,error)=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let found:Option<(i64,String,Option<String>,Option<String>)>=tx.query_row("SELECT id,confirmation_token,confirmed_at,unconfirmed_email FROM users WHERE confirmation_token=?1",[&token],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((id,stored,confirmed,pending))=found else{return Ok((false,false,None));};
        if token.is_empty()||!bool::from(stored.as_bytes().ct_eq(token.as_bytes())){return Ok((false,false,None));}
        let pending=pending.filter(|s|!crate::ruby::blank(s));
        if confirmed.is_some()&&pending.is_none(){return Ok((false,true,Some("errors.messages.already_confirmed")));}
        if let Some(email)=pending{
            let email=crate::ruby::strip(&email).to_lowercase();
            if let Some(error)=super::account::confirmation_validation(&tx,&email,id)?{return Ok((false,true,Some(error)));}
            tx.execute("UPDATE users SET email=?1,unconfirmed_email=NULL,confirmed_at=?2,updated_at=?2 WHERE id=?3",(email,format_time(inner.now()),id))?;
        }else{
            tx.execute("UPDATE users SET confirmed_at=?1,updated_at=?1 WHERE id=?2",(format_time(inner.now()),id))?;
        }
        tx.commit()?;Ok((true,true,None))
    }).await?;
    if success{
        flash::set(&ctx.session,flash::NOTICE,ctx.t("devise.confirmations.confirmed"));
        Ok(layout::redirect(StatusCode::FOUND,&ctx.path(if ctx.user().is_some(){"/"}else{"/login"})))
    }else{
        let error=error.map(|key|{let text=i18n::text(ctx.locale,key,&[]);let mut chars=text.chars();chars.next().map(|ch|ch.to_uppercase().collect::<String>()+chars.as_str()).unwrap_or_default()});
        form(&app,&ctx,false,persisted,error).await
    }
}

/// Devise's resend action always gives the same privacy response, including unknown addresses.
pub async fn create(State(app):State<App>,Extension(ctx):Extension<Ctx>)->Result<Response,WebError>{
    if !ctx.params.form.iter().any(|(name,_)|name.starts_with("user[")){return Ok(layout::redirect(StatusCode::SEE_OTHER,&ctx.path("/login")))}
    let address=crate::ruby::strip(ctx.params.form("user[email]").unwrap_or_default()).to_lowercase();
    let inner=app.clone();
    let delivery=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        type Pending=(i64,String,Option<String>,Option<String>,Option<String>);
        let found:Option<Pending>=tx.query_row("SELECT id,email,confirmed_at,unconfirmed_email,confirmation_token FROM users WHERE unconfirmed_email=?1 OR email=?1 ORDER BY CASE WHEN unconfirmed_email=?1 THEN 0 ELSE 1 END,id LIMIT 1",[&address],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((id,email,confirmed,pending,stored))=found else{return Ok(None)};
        let pending=pending.filter(|v|!crate::ruby::blank(v));
        if confirmed.is_some()&&pending.is_none(){return Ok(None)}
        let token=match stored{Some(token)=>token,None=>{
            use base64::{Engine,engine::general_purpose::URL_SAFE_NO_PAD};
            let token=URL_SAFE_NO_PAD.encode(rand::random::<[u8;15]>()).replace('l',"s").replace('I',"x").replace('O',"y").replace('0',"z");
            tx.execute("UPDATE users SET confirmation_token=?1,confirmation_sent_at=?2,updated_at=?2 WHERE id=?3",(&token,format_time(inner.now()),id))?;token
        }};
        tx.commit()?;Ok(Some((id,pending.unwrap_or(email),token)))
    }).await?;
    if let Some((id,address,token))=delivery{super::mail::confirmation(&app,id,address,token,ctx.locale).await?;}
    flash::set(&ctx.session,flash::NOTICE,ctx.t("devise.confirmations.send_paranoid_instructions"));
    Ok(layout::redirect(StatusCode::SEE_OTHER,&ctx.path("/login")))
}
