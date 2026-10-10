use super::view;
use crate::{
    codec::format_time,
    crypto::hash_password,
    web::{
        auth::{self, User},
        flash, i18n,
        layout::Ctx,
        turbo, App, WebError,
    },
};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use rusqlite::{Transaction, TransactionBehavior};

#[derive(Default)]
pub struct Draft {
    pub name: Option<String>,
    pub email: Option<String>,
    pub errors: Vec<String>,
    pub messages: std::collections::BTreeMap<String, String>,
}
fn matches(pattern: &str, value: &str) -> Result<bool, WebError> {
    Ok(crate::ruby::validation_regex(pattern)
        .map_err(|_| WebError::Config("invalid account validation pattern".into()))?
        .is_match(value))
}
/// Rails' Unicode string length, ASCII regexp classes and non-newline dot semantics.
fn password_complexity(password:&str)->bool{
    password.split('\n').any(|line|line.chars().count()>=8&&line.chars().any(|c|c.is_ascii_lowercase())&&line.chars().any(|c|c.is_ascii_uppercase())&&line.chars().any(|c|c.is_ascii_digit())&&line.chars().any(|c|!c.is_ascii_alphanumeric()))
}
/// User::Email compares Google usernames without plus tags, and preserves dots.
pub fn email_taken(c:&rusqlite::Connection,email:&str,user:i64)->Result<bool,WebError>{
    if c.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE email=?1 AND id<>?2)",(email,user),|r|r.get::<_,bool>(0))?{return Ok(true)}
    let google=|email:&str|if email.ends_with("@gmail.com")||email.ends_with("@googlemail.com"){email.split('@').next().map(|s|s.split('+').next().unwrap_or(s).to_lowercase())}else{None};
    let Some(wanted)=google(email) else{return Ok(false)};
    let mut stmt=c.prepare("SELECT email FROM users WHERE id<>?1 AND (email LIKE '%@gmail.com' OR email LIKE '%@googlemail.com')")?;
    let stored=stmt.query_map([user],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    Ok(stored.iter().any(|email|google(email).as_deref()==Some(wanted.as_str())))
}
fn stream(body: String) -> Response {
    ([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response()
}
pub async fn write(app: App, ctx: Ctx, _headers: HeaderMap) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    if !ctx
        .params
        .form
        .iter()
        .any(|(name, _)| name.starts_with("user["))
    {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let action = ctx
        .params
        .route_path
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let checked = if action == "update_email" || action == "update_password" {
        app.password_check(
            ctx.params
                .form("user[current_password]")
                .unwrap_or("")
                .to_string(),
            Some(user.encrypted_password.clone()),
        )
        .await?
    } else {
        Some(true)
    };
    let Some(correct) = checked else {
        return Ok(crate::web::rate_limit::throttled(1));
    };
    let view = ctx.clone();
    let old_hash = user.encrypted_password.clone();
    let result=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let Some(current)=User::find(&tx,user.id)? else{return Ok((Draft::default(),None,None,None));};
        // A verification result never authorizes a password that changed while bcrypt ran.
        let correct=correct&&bool::from(subtle::ConstantTimeEq::ct_eq(current.encrypted_password.as_bytes(),old_hash.as_bytes()));
        let now=format_time(view.app.now());
        let mut draft=Draft::default();
        let mut mail=None;
        let mut salt=None;
        let mut notice=None;
        match action.as_str(){
            "update_name"=>{
                if let Some(name)=view.params.form("user[name]"){
                    let previous:Option<String>=tx.query_row("SELECT name FROM users WHERE id=?1",[user.id],|r|r.get(0))?;
                    draft.name=Some(name.into());
                    if previous.as_deref()!=Some(name)&&!matches(r"(?m)^\p{L}+(\s+\p{L}+)*$",name)?{draft.errors.push("name".into());}
                    else if previous.as_deref()!=Some(name){tx.execute("UPDATE users SET name=?1,updated_at=?2 WHERE id=?3",(name,&now,user.id))?;}
                }
                notice=Some("settings.name.updated");
            }
            "update_time_zone"=>{
                if let Some(value)=view.params.form("user[time_zone]"){
                    let zone=if crate::ruby::blank(value){"UTC"}else{value};
                    if crate::web::timezone::zone(zone).is_none(){draft.errors.push("time_zone".into());}
                    else if zone!=current.time_zone {tx.execute("UPDATE users SET time_zone=?1,updated_at=?2 WHERE id=?3",(zone,&now,user.id))?;}
                }
                notice=Some("settings.language_and_timezone.updated");
            }
            "update_locale"=>{
                if let Some(value)=view.params.form("user[locale]"){
                    let locale=if crate::ruby::blank(value){None}else{Some(value)};
                    if locale.is_some_and(|v|crate::web::locale::known(v).is_none()){draft.errors.push("locale".into());}
                    else if locale!=current.locale.as_deref(){tx.execute("UPDATE users SET locale=?1,updated_at=?2 WHERE id=?3",(locale,&now,user.id))?;}
                }
                notice=Some("settings.language_and_timezone.language_updated");
            }
            "update_password"=>{
                let password=view.params.form("user[password]").filter(|p|!crate::ruby::blank(p)).unwrap_or("");
                if !password.is_empty(){
                    if password.chars().count()<8||password.chars().count()>128||!password_complexity(password){draft.errors.push("password".into());}
                    if view.params.form("user[password_confirmation]").is_some_and(|confirm|confirm!=password){draft.errors.push("password_confirmation".into());}
                }
                if password.is_empty()&&view.params.form("user[password_confirmation]").is_some_and(|p|!crate::ruby::blank(p)){draft.errors.push("password".into());draft.errors.push("password_confirmation".into());}
                if !correct{draft.errors.push("current_password".into());}
                if draft.errors.is_empty()&&!password.is_empty(){
                    let encrypted=hash_password(password).map_err(|_|WebError::Config("password hashing failed".into()))?;
                    tx.execute("UPDATE users SET encrypted_password=?1,updated_at=?2 WHERE id=?3",(&encrypted,&now,user.id))?;
                    salt=Some(encrypted.get(..29).unwrap_or(&encrypted).to_string());
                }
                notice=Some("devise.passwords.updated");
            }
            "update_email"=>{
                let email=match view.params.form("user[email]"){Some(email)=>email,None=>&current.email};
                let email=crate::ruby::strip(email).to_lowercase();
                draft.email=Some(email.clone());
                if email.is_empty()||!matches(r"\A[^@\s]+@[^@\s]+\z",&email)?||!matches(r"(?m)^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$",&email)?{draft.errors.push("email".into());}
                if !correct{draft.errors.push("current_password".into());}
                if draft.errors.is_empty()&&email!=current.email{
                    let taken=email_taken(&tx,&email,user.id)?;
                    if taken{
                        tx.execute("UPDATE users SET unconfirmed_email=?1,confirmation_token=NULL,confirmation_sent_at=NULL,updated_at=?2 WHERE id=?3",(view.params.form("user[email]").unwrap_or(&email),&now,user.id))?;
                    }else{
                        use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
                        let bytes:[u8;15]=rand::random();
                        let token=URL_SAFE_NO_PAD.encode(bytes).replace('l',"s").replace('I',"x").replace('O',"y").replace('0',"z");
                        tx.execute("UPDATE users SET unconfirmed_email=?1,confirmation_token=?2,confirmation_sent_at=?3,updated_at=?3 WHERE id=?4",(&email,&token,&now,user.id))?;
                        mail=Some((email,token));
                    }
                }
                notice=Some("devise.registrations.update_needs_confirmation");
            }
            _=>{}
        }
        for field in &draft.errors {
            let message=match field.as_str(){
              "name"=>i18n::text(view.locale,"devise.registrations.new.name_invalid",&[]),
              "email"=>{
                let email=draft.email.as_deref().unwrap_or("");let mut messages=vec![];
                if email.is_empty(){messages.push(i18n::text(view.locale,"errors.messages.blank",&[]));}
                else if !matches(r"\A[^@\s]+@[^@\s]+\z",email)?{messages.push(i18n::text(view.locale,"errors.messages.invalid",&[]));}
                if !matches(r"(?m)^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$",email)?{messages.push(i18n::text(view.locale,"devise.registrations.new.email_invalid",&[]));}
                messages.join(", ")
              },
              "password_confirmation"=>{
                let key=format!("{}.activerecord.attributes.user.password",view.locale);
                let attribute=i18n::all().iter().find(|(name,_)|*name==key).map_or("Password",|(_,text)|*text);
                i18n::text(view.locale,"errors.messages.confirmation",&[("attribute",i18n::Arg::Text(attribute))])
              },
              "password"=>{
                let mut messages=vec![];let p=view.params.form("user[password]").unwrap_or("");
                if crate::ruby::blank(p){i18n::text(view.locale,"errors.messages.blank",&[])}else{let n=p.chars().count();
                if n<8{messages.push(i18n::text(view.locale,"errors.messages.too_short",&[("count",i18n::Arg::Count(8))]));}
                if n>128{messages.push(i18n::text(view.locale,"errors.messages.too_long",&[("count",i18n::Arg::Count(128))]));}
                let p=view.params.form("user[password]").unwrap_or("");
                if !password_complexity(p){messages.push(i18n::text(view.locale,"errors.messages.too_simple_password",&[]));}
                messages.join(", ")}
              },
              "current_password"=>i18n::text(view.locale,if view.params.form("user[current_password]").is_none_or(crate::ruby::blank){"errors.messages.blank"}else{"errors.messages.invalid"},&[]),
              _=>i18n::text(view.locale,"errors.messages.inclusion",&[])
            };
            let mut chars=message.chars();let capitalized=chars.next().map(|ch|ch.to_uppercase().collect::<String>()+chars.as_str()).unwrap_or_default();
            draft.messages.insert(field.clone(),capitalized);
        }
        if draft.errors.is_empty(){tx.commit()?;}else{tx.rollback()?;}
        Ok((draft,salt,notice,mail))
    }).await?;
    let (draft, salt, notice, mail) = result;
    if let Some((email, token)) = mail {
        super::mail::confirmation(&app, user.id, email, token, ctx.locale).await?;
    }
    if let Some(salt) = salt {
        ctx.session.lock().user = Some((user.id, salt));
    }
    if !draft.errors.is_empty() {
        if draft
            .errors
            .iter()
            .any(|s| s == "locale" || s == "time_zone")
        {
            let message = ctx.t("errors.messages.inclusion");
            let flash = flash::render(&flash::take(&ctx.session, &[(flash::ALERT, message)]))?;
            let mut response = stream(turbo::prepend_flash(flash.trim_end()));
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return Ok(response);
        }
        return view::account(
            &app,
            &ctx,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(draft),
            vec![],
        )
        .await;
    }
    let Some(notice) = notice else {
        return Ok(crate::web::layout::missing());
    };
    if ctx.params.route_path == "/settings/update_name" {
        let flash = flash::render(&flash::take(
            &ctx.session,
            &[(flash::NOTICE, ctx.t(notice))],
        ))?;
        Ok(stream(turbo::prepend_flash(flash.trim_end())))
    } else if ctx.params.route_path == "/settings/update_locale" {
        let id = user.id;
        let locale = app
            .db(move |c| Ok(User::find(c, id)?.and_then(|u| u.locale)))
            .await?;
        let locale = locale
            .as_deref()
            .and_then(crate::web::locale::known)
            .unwrap_or(i18n::DEFAULT);
        flash::set(&ctx.session, flash::NOTICE, i18n::text(locale, notice, &[]));
        Ok(stream(turbo::redirect(&crate::web::locale::path(
            locale,
            "/settings/account",
        ))))
    } else {
        flash::set(&ctx.session, flash::NOTICE, ctx.t(notice));
        Ok(stream(turbo::refresh().into()))
    }
}

/// User save validations at confirmation, after Devise's normalization and before any row effect.
pub fn confirmation_validation(c:&rusqlite::Connection,email:&str,id:i64)->Result<Option<&'static str>,WebError>{
    if email.is_empty(){return Ok(Some("errors.messages.blank"));}
    if !matches(r"\A[^@\s]+@[^@\s]+\z",email)?{return Ok(Some("errors.messages.invalid"));}
    if email_taken(c,email,id)?{return Ok(Some("errors.messages.taken"));}
    if !matches(r"(?m)^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$",email)?{return Ok(Some("devise.registrations.new.email_invalid"));}
    let user=auth::User::find(c,id)?.ok_or_else(||WebError::Config("confirmation user unavailable".into()))?;
    if crate::web::timezone::zone(&user.time_zone).is_none()||user.locale.as_deref().is_some_and(|locale|crate::web::locale::known(locale).is_none())||!["USD","EUR","GBP","CHF","PLN"].contains(&user.display_currency.as_str()){
        return Ok(Some("errors.messages.inclusion"));
    }
    let jurisdiction:Option<String>=c.query_row("SELECT wash_sale_jurisdiction FROM users WHERE id=?1",[id],|r|r.get(0))?;
    if jurisdiction.as_deref().is_some_and(|v|!crate::ruby::blank(v)&&!["US","GB","IE"].contains(&v)){
        return Ok(Some("errors.messages.inclusion"));
    }
    Ok(None)
}
