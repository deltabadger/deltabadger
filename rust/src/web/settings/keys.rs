//! Credential saves validate outside the write lock, recheck ownership/state inside it, and guard before commit.
use crate::{
    codec::format_time,
    engine::eligibility,
    web::{auth, flash, i18n, layout::{self,Ctx}, turbo, App, WebError},
};
use askama::Template;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::Arc;
use subtle::ConstantTimeEq;
pub trait Logger: Send + Sync {
    fn warn(&self, line: &str);
}
struct Stderr;
impl Logger for Stderr {
    fn warn(&self, line: &str) {
        eprintln!("{line}");
    }
}
pub fn logger() -> Arc<dyn Logger> {
    Arc::new(Stderr)
}
pub fn scrub(text: &str, values: &[Option<String>]) -> String {
    let values:Vec<_>=values.iter().flatten().map(String::as_str).collect();
    crate::crypto::scrub_known(text,&values)
}
/// K: no new tracking. Read the intent and the engine's actual polling rows under the write lock.
pub const SETTLING: &str = "An order is still being settled. Please try again shortly.";
fn settling(c: &Connection, user: i64, exchange: i64) -> Result<bool, WebError> {
    let mut stmt = c.prepare("SELECT id FROM bots WHERE user_id=?1 AND exchange_id=?2 ORDER BY id")?;
    let ids = stmt.query_map((user, exchange), |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        let bot = crate::engine::model::load_bot(c, id)?;
        if bot.rust_placement().is_some() || !crate::engine::polling::waiting_ids(c, &bot)?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
fn refusal_text(ctx: &Ctx, reason: &str) -> String {
    if reason == SETTLING { reason.to_string() }
    else { i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(reason))]) }
}
#[derive(Clone)]
struct Key {
    id: Option<i64>,
    key: Option<String>,
    secret: Option<String>,
    passphrase: Option<String>,
    status: i64,
    last_error: Option<String>,
    updated_at: Option<String>,
    extras: [Option<String>;4],
    realm: Option<String>,
    version: Option<crate::engine::model::CredentialVersion>,
}
impl Key {
    fn read(
        c: &Connection,
        app: &App,
        user: i64,
        exchange: i64,
        kind: i64,
    ) -> Result<Self, WebError> {
        if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=Self::read(&tx,app,user,exchange,kind)?;tx.commit()?;return Ok(out)}
        let row=c.query_row("SELECT id,key,secret,passphrase,status,last_sync_error,updated_at,access_token,rsa_signature_key,rsa_encryption_key,dh_param,ibkr_realm FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=?3 ORDER BY id LIMIT 1",(user,exchange,kind),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?,[r.get::<_,Option<String>>(7)?,r.get(8)?,r.get(9)?,r.get(10)?],r.get::<_,Option<String>>(11)?))).optional()?;
        let decrypt = |text: Option<String>| {
            text.map(|s| {
                app.cipher
                    .decrypt(&s)
                    .map_err(|_| WebError::Config("unreadable exchange credential".into()))
            })
            .transpose()
        };
        match row {
            Some((id, key, secret, passphrase, status, last_error, updated_at, [a,b,extra_c,d], realm)) => Ok(Self {
                id: Some(id),
                key: decrypt(key)?,
                secret: decrypt(secret)?,
                passphrase: decrypt(passphrase)?,
                status,
                last_error,
                updated_at: Some(updated_at),
                extras:[decrypt(a)?,decrypt(b)?,decrypt(extra_c)?,decrypt(d)?],realm,
                version:crate::engine::model::credential_version_by_id(c,id)?,
            }),
            None => Ok(Self {
                id: None,
                key: None,
                secret: None,
                passphrase: None,
                status: 0,
                last_error: None,
                updated_at: None,
                extras: [None,None,None,None],realm:None,version:None,
            }),
        }
    }
    fn same(&self, other: &Self) -> bool {
        self.id == other.id
            && self.version == other.version
            && same(&self.key, &other.key)
            && same(&self.secret, &other.secret)
            && same(&self.passphrase, &other.passphrase)
            && self.extras.iter().zip(&other.extras).all(|(a,b)|same(a,b)) && self.realm==other.realm
    }
}
fn same(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => bool::from(a.as_bytes().ct_eq(b.as_bytes())),
        (None, None) => true,
        _ => false,
    }
}
fn flash_response(ctx: &Ctx, status: StatusCode, key: &str) -> Result<Response, WebError> {
    let message = i18n::text(ctx.locale, key, &[]);
    let body = turbo::prepend_flash(flash::render(&flash::take(
        &ctx.session,
        &[(flash::ALERT, message)],
    ))?.trim_end());
    Ok((status, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response())
}
use super::validator::Validity;
/// The prepared envelopes are the stored producer field for a newly validated slot.
/// The pending zero identity never enters any persisted account cache.
struct ValidatedKey {
    key:Key,
    columns:[Option<String>;7],
    handle:super::validator::ValidationHandle,
    result:crate::engine::model::Produced<Validity>,
}
async fn validate(app:&App,key:&Key,original:&Key,kind:i64)->Result<ValidatedKey,WebError>{
    let mut values=key.extras.iter().flatten().cloned().collect::<Vec<_>>();
    values.extend([original.key.as_ref(),original.secret.as_ref(),original.passphrase.as_ref()].into_iter().flatten().chain(original.extras.iter().flatten()).cloned());
    let columns=[key.key.as_ref(),key.secret.as_ref(),key.passphrase.as_ref(),key.extras[0].as_ref(),key.extras[1].as_ref(),key.extras[2].as_ref(),key.extras[3].as_ref()].map(|value|value.map(|value|app.cipher.encrypt(value)));
    let producer=crate::engine::model::prepared_credential_version(key.id.unwrap_or(0),&columns);
    let handle=super::validator::ValidationHandle::stored(crate::crypto::Credentials{redaction_values:values,key:key.key.clone().unwrap_or_default(),secret:key.secret.clone().unwrap_or_default(),passphrase:key.passphrase.clone()},producer);
    let result=handle.check(&app.alpaca_trading_url(handle.credentials()),kind).await?;
    app.observe_validation(result.origin().clone())?;
    Ok(ValidatedKey{key:key.clone(),columns,handle,result})
}
async fn tracker_exchange(app:&App,ctx:&Ctx)->Result<Option<i64>,WebError>{
    let requested=ctx.params.form("exchange_id").or_else(||ctx.params.query("exchange_id")).and_then(|s|s.parse::<i64>().ok());
    let saved=ctx.session.lock().tracker_connect;
    let found=app.db(move|c|{
        for id in [requested,saved].into_iter().flatten(){
            if c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get::<_,bool>(0))?{return Ok(Some(id))}
        }
        Ok(None)
    }).await?;
    Ok(found)
}
/// RetiredExchangeGuard: flash and back to the tracker, before any credential is read or a venue asked.
fn retired_answer(ctx:&Ctx)->Response{
    flash::set(&ctx.session,flash::ALERT,ctx.t("errors.exchange_retired"));
    crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker"))
}
pub async fn save(
    axum::extract::State(app): axum::extract::State<App>,
    axum::extract::Extension(ctx): axum::extract::Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let bot_request=ctx.params.route_path.starts_with("/bots/");
    let bot_exchange=if bot_request {owned_bot_exchange(&app,&ctx).await?} else {None};
    if bot_request && bot_exchange.is_none(){return Ok(crate::web::layout::missing())}
    let exchange=if bot_request{bot_exchange}else{tracker_exchange(&app,&ctx).await?};
    let Some(exchange) = exchange else {
        return Ok(crate::web::layout::redirect(
            StatusCode::FOUND,
            &ctx.path("/tracker/pick_exchange/new"),
        ));
    };
    let kind = if bot_request {0} else {match ctx
        .params
        .form("key_type")
        .or_else(|| ctx.params.query("key_type"))
    {
        Some("trading") => 0,
        _ => 2,
    }};
    let inner = app.clone();
    let id = user.id;
    let found = app
        .db(move |c| {
            let venue = c
                .query_row("SELECT type FROM exchanges WHERE id=?1", [exchange], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?;
            Ok((venue, Key::read(c, &inner, id, exchange, kind)?))
        })
        .await?;
    let (venue, original) = found;
    if !bot_request && venue.as_deref().is_some_and(crate::web::bot::retired_type) {
        ctx.session.lock().tracker_connect=Some(exchange);
        return Ok(retired_answer(&ctx));
    }
    if !ctx.params.form.iter().any(|(name, _)| name.starts_with("api_key[")) {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    if venue.as_deref() == Some("Exchanges::Alpaca") && !bot_request {ctx.session.lock().tracker_connect=Some(exchange);}
    if venue.as_deref() != Some("Exchanges::Alpaca") {
        return Ok(crate::web::layout::not_ported_response(
            &ctx.method,
            &ctx.params.fullpath,
            ctx.turbo_frame.as_deref(),
        ));
    }
    let mut candidate = original.clone();
    if let Some(value) = ctx.params.form("api_key[key]") {
        candidate.key = Some(value.into());
    }
    if let Some(value) = ctx.params.form("api_key[secret]") {
        candidate.secret = Some(value.into());
    }
    if let Some(value) = ctx.params.form("api_key[passphrase]") {
        candidate.passphrase = Some(value.into());
    }
    for (i,name) in ["access_token","rsa_signature_key","rsa_encryption_key","dh_param"].iter().enumerate(){
        if let Some(value)=ctx.params.form(&format!("api_key[{name}]")){candidate.extras[i]=Some(value.into());}
    }
    if let Some(value)=ctx.params.form("api_key[ibkr_realm]"){candidate.realm=Some(value.into());}
    let validated=validate(&app,&candidate,&original,kind).await?;
    match &validated.result.value {
        Validity::Incorrect => {
            return flash_response(
                &ctx,
                StatusCode::UNPROCESSABLE_ENTITY,
                "errors.incorrect_api_key_permissions",
            )
        }
        Validity::Pending(diagnostic) => {
            let mut values = vec![
                candidate.key.clone(),
                candidate.secret.clone(),
                candidate.passphrase.clone(),
                original.key.clone(),
                original.secret.clone(),
                original.passphrase.clone(),
            ];
            values.extend(candidate.extras.iter().cloned());values.extend(original.extras.iter().cloned());
            let redaction_material = values;
            app.settings_key_logger.warn(&format!(
                "[Alpaca] API key validation failed: {}",
                scrub(&diagnostic.log_text(),&redaction_material)
            ));
            return flash_response(
                &ctx,
                StatusCode::UNPROCESSABLE_ENTITY,
                "errors.api_key_permission_validation_failed",
            );
        }
        Validity::Correct => {}
    }
    let jobs=app.job_wakers()?;
    let inner = app.clone();
    let outcome=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let current=Key::read(&tx,&inner,user.id,exchange,kind)?;
  if !original.same(&current){return Ok(Some("credential changed during validation".into()));}
  if !validated.result.current_for(&Some(validated.handle.producer().clone())).is_fresh(){return Ok(Some("credential changed during validation".into()));}
  let candidate=&validated.key;
  let now=format_time(inner.now());
  let changed=!(same(&candidate.key,&current.key)&&same(&candidate.secret,&current.secret)&&same(&candidate.passphrase,&current.passphrase)&&candidate.extras.iter().zip(&current.extras).all(|(a,b)|same(a,b))&&candidate.realm==current.realm&&current.status==1&&current.last_error.is_none());
  if current.id.is_some() && settling(&tx,user.id,exchange)? { return Ok(Some(SETTLING.into())); }
  {
   let now=if changed {now} else {current.updated_at.clone().unwrap_or(now)};
   let key=validated.columns[0].clone();let secret=validated.columns[1].clone();let passphrase=validated.columns[2].clone();
   match current.id{
    Some(id)=>{tx.execute("UPDATE api_keys SET key=?1,secret=?2,passphrase=?3,status=1,last_sync_error=NULL,updated_at=?4 WHERE id=?5 AND user_id=?6",(key,secret,passphrase,&now,id,user.id))?;},
    None=>{tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7)",(user.id,exchange,kind,key,secret,passphrase,&now))?;}
   }
  }
  // #497: another credential may be another account, which syncs its own full history; its rows stay linked.
  if let Some(id)=current.id.filter(|_|!(same(&candidate.key,&current.key)&&same(&candidate.secret,&current.secret))){
   tx.execute("UPDATE api_keys SET last_synced_at=NULL WHERE id=?1",[id])?;
   tx.execute("DELETE FROM app_configs WHERE key IN (?1,?2)",(format!("rust_sync.ledger:{id}"),format!("rust_sync.ledger_splits:{id}")))?;
  }
  tx.execute("UPDATE api_keys SET access_token=?1,rsa_signature_key=?2,rsa_encryption_key=?3,dh_param=?4,ibkr_realm=?5 WHERE user_id=?6 AND exchange_id=?7 AND key_type=?8",rusqlite::params![validated.columns[3],validated.columns[4],validated.columns[5],validated.columns[6],candidate.realm,user.id,exchange,kind])?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()));}
  let id:i64=tx.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=?3",(user.id,exchange,kind),|r|r.get(0))?;
  // The slots a restart would register (sync::jobs::register, tracker::jobs::register): a key that reads for its
  // venue gets its syncs, and its owner the tracker's jobs. Woken after the commit only.
  let reading=crate::sync::reading_keys(&tx).map_err(|e|WebError::Config(e.0))?.contains(&id);
  tx.commit()?;
  if kind==0 {inner.wake_engine();}
  if reading {
   for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] {jobs.wake(job,Some(&id.to_string()),None);}
   for job in [crate::tracker::jobs::TRACKER_LEDGER,crate::tracker::jobs::PORTFOLIO_BACKFILL] {jobs.wake(job,Some(&user.id.to_string()),None);}
  }
  Ok(None)
 }).await?;
    if let Some(reason) = outcome {
        let text = refusal_text(&ctx, &reason);
        let body = turbo::prepend_flash(&flash::render(&flash::take(
            &ctx.session,
            &[(flash::ALERT, text)],
        ))?);
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            body,
        )
            .into_response());
    }
    if bot_request {
        flash::set(&ctx.session,flash::NOTICE,ctx.t("errors.bots.api_key_success"));
        return Ok(([(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],turbo::refresh()).into_response());
    }
    ctx.session.lock().tracker_connect=None;
    // The warnings widget is rebuilt from persisted rows, as in Tracker::AddApiKeysController.
    let v=ctx.clone();let owner=ctx.user().ok_or_else(||WebError::Config("missing warning owner".into()))?.id;
    let warnings = app.db(move|c|warnings(c,&v,owner)).await?;
    let body = turbo::stream("update", "sync-warnings", &warnings)
        + &turbo::redirect(&ctx.path("/tracker"));
    Ok(([(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], body).into_response())
}

#[derive(Template)]
#[template(path="settings/sync_warnings.html")]
struct Warnings{title:String,fixes:Vec<Fix>}
struct Fix{message:String,trading:bool,trading_path:String,reading_path:String,trading_label:String,add_label:String,replace_label:String}
struct WarningKey{exchange:i64,name:String,venue:String,status:i64,kind:i64,error:Option<String>}
impl WarningKey{
    fn failed(&self)->bool{self.error.as_deref().is_some_and(|e|!crate::ruby::blank(e))}
    fn permission(&self)->bool{self.venue=="Exchanges::Kraken"&&self.error.as_deref().is_some_and(|e|e.contains("EGeneral:Permission denied"))}
}
/// `ApiKey.sync_warnings` rendered as tracker/_sync_warnings: the contents of #sync-warnings.
pub(crate) fn warnings(c:&Connection,v:&Ctx,user:i64)->Result<String,WebError>{
    let mut statement=c.prepare("SELECT k.exchange_id,e.name,e.type,k.status,k.key_type,k.last_sync_error FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type!=1 ORDER BY k.id")?;
    let keys=statement.query_map([user],|r|Ok(WarningKey{exchange:r.get(0)?,name:r.get(1)?,venue:r.get(2)?,status:r.get(3)?,kind:r.get(4)?,error:r.get(5)?}))?.collect::<Result<Vec<_>,_>>()?;
    let mut failed=vec![];let mut seen=std::collections::HashSet::new();
    for key in &keys{
        if !seen.insert(key.exchange){continue}
        let group:Vec<_>=keys.iter().filter(|k|k.exchange==key.exchange).collect();
        let reading=group.iter().copied().find(|k|k.status==1&&k.kind==0&&!k.permission()).or_else(||group.iter().copied().find(|k|k.status==1&&k.kind==2)).or_else(||group.iter().copied().find(|k|k.status==1));
        let chosen=if let Some(reading)=reading{reading.failed().then_some(reading)}else{group.iter().copied().find(|k|k.kind==2&&k.failed()).or_else(||group.iter().copied().find(|k|k.failed()))};
        if let Some(key)=chosen{failed.push(key)}
    }
    if failed.is_empty(){return Ok(String::new())}
    let names=failed.iter().map(|k|k.name.as_str()).collect::<Vec<_>>().join(", ");
    let mut fixes=vec![];
    for key in failed{
        let reason=if key.permission(){Some("missing_permission")}else if key.status==2{Some("dead")}else{None};
        if let Some(reason)=reason{
            let args=[("exchange",i18n::Arg::Text(&key.name))];let label=|name:&str|i18n::t(v.locale,name,&args);
            fixes.push(Fix{message:label(&format!("tracker.key_fix.{}.{reason}",if key.kind==0{"trading"}else{"read_only"})),trading:key.kind==0,trading_path:i18n::escape(&v.path(&format!("/tracker/add_api_key/new?exchange_id={}&key_type=trading",key.exchange))),reading_path:i18n::escape(&v.path(&format!("/tracker/add_api_key/new?exchange_id={}&key_type=read_only",key.exchange))),trading_label:label("tracker.replace_trading_key"),add_label:label("tracker.add_tracker_key"),replace_label:label("tracker.replace_tracker_key")});
        }
    }
    Ok(Warnings{title:i18n::t(v.locale,"tracker.sync_failed_for",&[("exchanges",i18n::Arg::Text(&names))]),fixes}.render()?+"\n")
}

fn lookup(locale: &str, key: &str) -> Option<&'static str> {
    for language in [locale, i18n::DEFAULT] {
        let key = format!("{language}.{key}");
        if let Some((_, text)) = i18n::all().iter().find(|(name, _)| *name == key) {
            return Some(*text);
        }
    }
    None
}
/// settings/widgets/_api_keys: the exchange keys, a row each; removing one asks first.
pub fn list(c: &Connection, ctx: &Ctx, csrf: &str, user: i64) -> Result<String, WebError> {
    use super::view::{confirm, row};
    let mut s=c.prepare("SELECT k.id,e.name,e.type,k.key_type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 ORDER BY k.id")?;
    let rows = s
        .query_map([user], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut body = String::new();
    for (index, (heading, withdrawal)) in [("settings.index.trading_keys_section", false), ("settings.index.withdrawal_keys_section", true)].into_iter().enumerate() {
        let group: Vec<_> = rows.iter().filter(|(_, _, _, kind)| (*kind == 1) == withdrawal).collect();
        if index > 0 && group.is_empty() { continue; }
        body += &format!("\n    <h3 class=\"settings__heading {}\">{}</h3>\n", if index == 0 { "settings__heading--first" } else { "" }, ctx.t(heading));
        if group.is_empty() {
            body += &format!("      <p class=\"settings__empty\">{}</p>\n", ctx.t("settings.sections.no_keys"));
        }
        for (id, name, venue, kind) in group {
            let name_id = crate::tracker::rows::name_id(venue);
            let prefix = if *kind == 1 { "withdrawal_api" } else { "bot.api" };
            let instructions = lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions")).is_some()
                || lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions.0.text_html")).is_some();
            let label = format!("{}{}", i18n::escape(name), if *kind == 2 { format!("<span class=\"srow__aside\">{}</span>", ctx.t("links.tracker")) } else { String::new() });
            let mut control = String::new();
            if instructions {
                control += &format!(
                    "      <a class=\"rbutton rbutton--outline\" data-turbo-frame=\"settings_pane\" href=\"{}\">{}</a>\n        ",
                    i18n::escape(&ctx.path(&format!("/settings/api_key_permissions/{id}"))),
                    ctx.t("settings.sections.permissions")
                );
            }
            control += &confirm(csrf, &ctx.t("button.remove"), &ctx.t("settings.api_keys.confirm_delete"), &ctx.t("button.delete"), &ctx.path(&format!("/settings/destroy_api_key/{id}")), true, &[]);
            body += &format!("      {}", row(&label, None, Some(crate::web::bot::exchange_svg(&name_id)), "srow--key", &format!(" data-api-key=\"{id}\""), Some(&control)));
        }
    }
    Ok(format!("<turbo-frame id=\"api_keys\">{body}</turbo-frame>\n"))
}
/// settings/widgets/_stocks and settings/widgets/_market_data, as AppConfig and the catalog hold them now.
pub(super) fn connection_widgets(c:&Connection,app:&App,v:&Ctx,csrf:&str)->Result<(String,String),WebError>{
    use super::view::{confirm, row, switch};
    use crate::web::bots::{Segmented, SegmentedOption};
    let setting=|name:&str|auth::app_config(c,&app.cipher,name);
    let has=|name:&str|Ok::<_,WebError>(setting(name)?.is_some_and(|s|!crate::ruby::blank(&s)));
    let form=|attrs:&str,path:&str,patch:bool|format!("<form{attrs} action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">{}<input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />",i18n::escape(&v.path(path)),if patch{"<input type=\"hidden\" name=\"_method\" value=\"patch\" />"}else{""});
    let submit_connect=format!("<input type=\"submit\" name=\"commit\" value=\"{0}\" class=\"rbutton rbutton--outline\" data-turbo-stream=\"true\" data-turbo-submits-with=\"{1}\" data-disable-with=\"{0}\" />",v.t("button.connect"),v.t("bot.setup.validating"));
    let help=|name:&str,prose:&str,indent:&str|format!("{indent}<details class=\"shelp\">\n{indent}  <summary>{}</summary>\n{indent}  <div class=\"settings__prose\">{prose}</div>\n{indent}</details>",i18n::t(v.locale,"bot.setup.how_to_get_keys",&[("exchange",i18n::Arg::Text(name))]));
    let disconnect=|label_key:&str,path:&str|{let label=v.t(label_key);confirm(csrf,&label,&v.t("settings.index.api_key_confirmation"),&label,&v.path(path),true,&[])};
    let hosted_note=|key:&str|i18n::t(v.locale,key,&[("provider_name",i18n::Arg::Text(&app.settings_market_provider_name))]);
    let provider=setting("market_data_provider")?.unwrap_or_default();let hosted=app.config.market_data_url;
    let current=if hosted{"deltabadger".to_string()}else{provider};
    let (coingecko,deltabadger)=(current=="coingecko",current=="deltabadger");
    let platform_connected=has("platform_connected_at")?;
    // Stocks.
    let stocks_body=if deltabadger{
        format!("\n    {}\n",row("Alpaca",Some(&hosted_note("settings.stocks.deltabadger_configured")),None,"","",None))
    }else{
        let configured=has("alpaca_api_key")?&&has("alpaca_api_secret")?;
        let stored_mode=setting("alpaca_mode")?;let mode=if stored_mode.as_deref()==Some("paper"){"paper"}else{"live"};
        let active=c.query_row("SELECT EXISTS(SELECT 1 FROM tickers t JOIN exchanges e ON e.id=t.exchange_id WHERE t.available=1 AND e.type IN('Exchanges::Alpaca','Exchanges::Ibkr'))",[],|r|r.get::<_,bool>(0))?;
        let hint=if configured{
            let display=stored_mode.map(|mode|{let mut chars=mode.chars();chars.next().map(|ch|ch.to_uppercase().collect::<String>()+&chars.as_str().to_lowercase()).unwrap_or_default()}).unwrap_or_else(||"Paper".into());
            Some(i18n::t(v.locale,"settings.stocks.configured",&[("mode",i18n::Arg::Text(&display))]))
        }else if active{Some(v.t("settings.stocks.not_refreshing"))}else{None};
        let control=if configured{format!("    {}",disconnect("settings.stocks.disconnect","/settings/disconnect_stocks"))}else{String::new()};
        let segmented=Segmented{fluid:false,label:"Mode".into(),key:None,links:false,options:vec![
            SegmentedOption{value:"paper",label:v.t("settings.stocks.mode_paper"),active:mode=="paper",href:None},
            SegmentedOption{value:"live",label:v.t("settings.stocks.mode_live"),active:mode=="live",href:None}]}.render()?;
        let input=|name:&str,label:&str|format!("<input autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{label}\" aria-label=\"{label}\" type=\"text\" name=\"{name}\" id=\"{name}\" />");
        format!("\n    {}\n\n    {}\n      {}      <input value=\"{mode}\" data-form--segmented-field-target=\"field\" type=\"hidden\" name=\"alpaca_mode\" id=\"alpaca_mode\" />\n      {}\n      {}\n      {submit_connect}\n{}\n</form>",
            row("Alpaca",hint.as_deref(),None,"","",Some(&control)),
            form(" class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"stocks_settings\" data-controller=\"form--segmented-field\" data-action=\"segmented:change-&gt;form--segmented-field#set\"","/settings/update_stocks",true),
            row("Mode",None,None,"srow--flush","",Some(&format!("\n{segmented}\n"))),
            input("alpaca_api_key","API Key"),input("alpaca_api_secret","API Secret"),
            help("Alpaca",&format!("<p>{}</p>",lookup(v.locale,"bot.api.alpaca.instructions").unwrap_or("")),"      "))
    };
    let stocks=format!("<turbo-frame class=\"sitem\" id=\"stocks_settings\">{stocks_body}</turbo-frame>\n");
    // Market data.
    let title=v.t("settings.index.resync_assets_section");
    let market_body=if hosted{
        format!("\n    {}\n",row(&title,Some(&hosted_note("settings.market_data.deltabadger_configured")),None,"srow--heading","",None))
    }else{
        let selected=if coingecko{"coingecko"}else if deltabadger{"deltabadger"}else{""};
        let on=!selected.is_empty();
        let enabled=if on{""}else if platform_connected{"deltabadger"}else{"coingecko"};
        let mut inner=format!("\n      {}\n",row(&title,None,None,"srow--heading","",Some(&switch(v,csrf,"/settings/update_market_data","market_data_settings","market_data_provider",if on{"on"}else{"off"},enabled,Some(&title),&[]))));
        if on{
            let provider=v.t("settings.sections.provider");
            let segmented=Segmented{fluid:true,label:provider.clone(),key:None,links:false,options:vec![
                SegmentedOption{value:"coingecko",label:"CoinGecko".into(),active:selected=="coingecko",href:None},
                SegmentedOption{value:"deltabadger",label:v.t("settings.platform.option"),active:selected=="deltabadger",href:None}]}.render()?;
            let provider_form=format!("{}\n            \n{segmented}\n            <input value=\"{selected}\" data-form--market-data-target=\"provider\" type=\"hidden\" name=\"market_data_provider\" id=\"market_data_provider\" />\n</form>",
                form(" data-form--market-data-target=\"providerForm\" data-turbo-frame=\"market_data_settings\"","/settings/update_market_data",true));
            inner+=&format!("        {}\n",row(&provider,None,None,"","",Some(&provider_form)));
            let coingecko_fields=if coingecko{
                let resync=format!("<form class=\"button_to\" method=\"post\" action=\"{}\"><button class=\"rbutton rbutton--outline\" type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" /></form>",i18n::escape(&v.path("/settings/resync_assets")),v.t("settings.resync_assets.button"));
                format!("\n            {}",row("CoinGecko",Some(&v.t("settings.market_data.coingecko_configured")),None,"","",Some(&format!("  {resync}\n              {}",disconnect("settings.market_data.disconnect","/settings/disconnect_market_data")))))
            }else if !has("coingecko_api_key")?{
                let key=v.t("setup.coingecko_api_key");
                format!("\n            {}\n              <input value=\"coingecko\" type=\"hidden\" name=\"market_data_provider\" id=\"market_data_provider\" />\n              <input required=\"required\" autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{key}\" aria-label=\"{key}\" type=\"text\" name=\"coingecko_api_key\" id=\"coingecko_api_key\" />\n              {submit_connect}\n{}\n</form>",
                    form(" class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"market_data_settings\"","/settings/update_market_data",true),
                    help("CoinGecko",&v.t("setup.coingecko_api_key_hint_html"),"              "))
            }else{String::new()};
            inner+=&format!("        <div class=\"sitem\" data-form--market-data-target=\"coingeckoFields\" {}>{coingecko_fields}        </div>\n\n",if selected=="coingecko"{""}else{"hidden"});
            let platform_form=if platform_connected{String::new()}else{
                let code=v.t("settings.platform.code");
                format!("\n            {}\n              <p class=\"srow__hint\">{}</p>\n              <input required=\"required\" autocomplete=\"off\" class=\"rinput rinput--outline rinput--text\" placeholder=\"{code}\" aria-label=\"{code}\" type=\"text\" name=\"claim_code\" id=\"claim_code\" />\n              {submit_connect}\n</form>",
                    form(" class=\"sform sform--stack sform--inset sitem\" data-turbo-frame=\"market_data_settings\"","/settings/platform_connection",false),
                    v.t("settings.platform.subscription_html"))
            };
            inner+=&format!("        <div class=\"sitem\" data-form--market-data-target=\"deltabadgerButtons\" {}>{platform_form}        </div>\n",if selected=="deltabadger"{""}else{"hidden"});
        }
        if platform_connected{
            let proxies:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE key LIKE 'proxy_%')",[],|r|r.get(0))?;
            inner+=&format!("\n        {}\n",row(&v.t("settings.platform.option"),Some(&v.t(if proxies{"settings.platform.connected_with_proxies"}else{"settings.platform.connected_without_proxies"})),None,"","",Some(&disconnect("settings.platform.disconnect","/settings/platform_connection"))));
        }
        format!("\n\n    <div data-controller=\"form--market-data\" data-action=\"segmented:change->form--market-data#select\"\n         data-form--market-data-platform-connected-value=\"{platform_connected}\">{inner}\n    </div>\n")
    };
    let market=format!("<turbo-frame id=\"market_data_settings\">{market_body}</turbo-frame>\n");
    Ok((stocks,market))
}
/// GET /settings/api_key_permissions/:id: how to get the key, inside the dialog.
pub async fn modal(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let Some(id) = ctx.params.route_path.rsplit('/').next().and_then(|s| s.parse::<i64>().ok()) else {
        return Ok(crate::web::layout::missing());
    };
    let found=app.db(move|c|Ok(c.query_row("SELECT e.name,e.type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1 AND k.user_id=?2",(id,user.id),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?)).await?;
    let Some((name, venue)) = found else {
        return Ok(crate::web::layout::missing());
    };
    if venue != "Exchanges::Alpaca" {
        return Ok(crate::web::layout::not_ported_response(&ctx.method, &ctx.params.fullpath, ctx.turbo_frame.as_deref()));
    }
    let heading = i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", i18n::Arg::Text(&name))]);
    let prose = format!("<p>{}</p>", lookup(ctx.locale, "bot.api.alpaca.instructions").unwrap_or(""));
    let pane = super::view::permissions_pane(&ctx, &heading, &prose);
    super::view::page(&app, &ctx, "api_keys", pane).await
}
pub async fn delete(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let Some(id) = ctx
        .params
        .route_path
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
    else {
        return Ok(crate::web::layout::missing());
    };
    let inner = app.clone();
    let v = ctx.clone();
    let csrf = ctx.csrf_token();
    let outcome=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let key:Option<(i64,i64)>=tx.query_row("SELECT exchange_id,key_type FROM api_keys WHERE id=?1 AND user_id=?2",(id,user.id),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;let Some((exchange,kind))=key else{return Ok(None);};
  if settling(&tx,user.id,exchange)? { return Ok(Some(Err(SETTLING.into()))); }
  let now=format_time(inner.now());
  if kind==0{
   let mut s=tx.prepare("SELECT id FROM bots WHERE user_id=?1 AND exchange_id=?2 AND status IN(1,4,5,6)")?;let ids=s.query_map((user.id,exchange),|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;drop(s);
   for bot in ids{tx.execute("UPDATE bots SET status=2,stopped_at=?1,stop_message_key=NULL,updated_at=?1 WHERE id=?2",(&now,bot))?;tx.execute("INSERT INTO bot_activity_logs(bot_id,event,level,message,details,created_at)VALUES(?1,'stopped',0,NULL,'{}',?2)",(bot,&now))?;}
  }
  tx.execute("UPDATE account_transactions SET api_key_id=NULL WHERE api_key_id=?1",[id])?;
  tx.execute("DELETE FROM api_keys WHERE id=?1 AND user_id=?2",(id,user.id))?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(Err(refusal.reason())));}
  let body=list(&tx,&v,&csrf,user.id)?;tx.commit()?;inner.wake_engine();Ok(Some(Ok(body)))
 }).await?;
    match outcome {
        None => Ok(crate::web::layout::missing()),
        Some(Ok(body)) => Ok((
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            turbo::stream("replace", "api_keys", body.trim_end()),
        )
            .into_response()),
        Some(Err(reason)) => {
            let text = refusal_text(&ctx, &reason);
            Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
                turbo::prepend_flash(
                    flash::render(&flash::take(&ctx.session, &[(flash::ALERT, text)]))?.trim_end(),
                ),
            )
                .into_response())
        }
    }
}
pub async fn legacy(
    axum::extract::State(app): axum::extract::State<App>,
    axum::extract::Extension(ctx): axum::extract::Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    // Rails reads `params.require(:api_key)` only: never a top-level `?secret=`. Its params merge the query string
    // and the body with the body winning, so `?api_key[name]=` counts only when the body has no value.
    let parameter = |name: &str| {
        let nested = format!("api_key[{name}]");
        ctx.params
            .form(&nested)
            .or_else(|| {
                ctx.params
                    .json
                    .as_ref()
                    .and_then(|j| j.get("api_key"))
                    .and_then(|root| root.get(name))
                    .and_then(|v| v.as_str())
            })
            .map(str::to_string)
            .or_else(||ctx.params.json.as_ref().and_then(|j|j.get("api_key")).and_then(|root|root.get(name)).filter(|value|value.is_number()||value.is_boolean()).map(serde_json::Value::to_string))
            .or_else(|| ctx.params.query(&nested).map(str::to_string))
    };
    if !ctx
        .params
        .form
        .iter()
        .any(|(name, _)| name.starts_with("api_key["))
        && ctx
            .params
            .json
            .as_ref()
            .and_then(|j| j.get("api_key"))
            .is_none()
    {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let exchange = parameter("exchange_id").and_then(|s| s.parse::<i64>().ok());
    let kind = match parameter("key_type").as_deref() {
        Some("withdrawal") => 1,
        Some("read_only") => 2,
        _ => 0,
    };
    let key = parameter("key").map(|s| s.replace("\\n", "\n"));
    let secret = parameter("secret").map(|s| s.replace("\\n", "\n"));
    let passphrase = parameter("passphrase");
    let Some(exchange) = exchange else {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
            serde_json::json!({"data":false}).to_string(),
        )
            .into_response());
    };
    let jobs = app.job_wakers()?;
    let inner = app.clone();
    let saved=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let venue:Option<String>=tx.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0)).optional()?;
  let Some(venue)=venue else{return Ok(Err(None));};
  let current=Key::read(&tx,&inner,user.id,exchange,kind)?;
  if current.id.is_some() && settling(&tx,user.id,exchange)? { return Ok(Err(Some(SETTLING.to_string()))); }
  let same_keys=same(&current.key,&key)&&same(&current.secret,&secret);
  if venue=="Exchanges::Hyperliquid"{
   let address=crate::ruby::validation_regex(r"\A0x[0-9a-fA-F]{40}\z").map_err(|_|WebError::Config("invalid wallet pattern".into()))?;
   let agent=crate::ruby::validation_regex(r"\A(0x)?[0-9a-fA-F]{64}\z").map_err(|_|WebError::Config("invalid agent pattern".into()))?;
   if !address.is_match(key.as_deref().unwrap_or(""))||!agent.is_match(secret.as_deref().unwrap_or("")){return Ok(Err(None));}
  }
  if venue!="Exchanges::Alpaca"{return Ok(Err(Some("credential venue is not ported".into())));}
  let now=format_time(inner.now());
  let saved_passphrase=if same_keys {passphrase.as_ref().or(current.passphrase.as_ref())}else{passphrase.as_ref()};
  let id=if let Some(id)=current.id{
   let stamp=if !same_keys||current.status!=0 {now.clone()} else {current.updated_at.clone().unwrap_or_else(||now.clone())};
   if same_keys {
    tx.execute("UPDATE api_keys SET status=0,updated_at=?1 WHERE id=?2",(&stamp,id))?;
   } else {
    tx.execute("UPDATE api_keys SET key=?1,secret=?2,passphrase=?3,status=0,last_sync_error=NULL,last_synced_at=NULL,updated_at=?4 WHERE id=?5",(key.as_ref().map(|s|inner.cipher.encrypt(s)),secret.as_ref().map(|s|inner.cipher.encrypt(s)),saved_passphrase.map(|s|inner.cipher.encrypt(s)),stamp,id))?;
    tx.execute("DELETE FROM app_configs WHERE key IN (?1,?2)",(format!("rust_sync.ledger:{id}"),format!("rust_sync.ledger_splits:{id}")))?;
   }
   id
  }else{
   tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,0,?7,?7)",(user.id,exchange,kind,key.as_ref().map(|s|inner.cipher.encrypt(s)),secret.as_ref().map(|s|inner.cipher.encrypt(s)),passphrase.as_ref().map(|s|inner.cipher.encrypt(s)),&now))?;tx.last_insert_rowid()
  };
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Err(Some(refusal.reason())));}
  tx.commit()?;inner.wake_engine();jobs.wake(crate::sync::jobs::API_KEY_VALIDATOR,Some(&id.to_string()),None);if !same_keys{jobs.wake(crate::sync::jobs::LEDGER_SYNC,Some(&id.to_string()),None);}Ok(Ok(()))
 }).await?;
    if matches!(&saved,Err(Some(reason)) if reason=="credential venue is not ported"){return Ok(layout::not_ported_response(&ctx.method,&ctx.params.fullpath,ctx.turbo_frame.as_deref()));}
    let (status, data) = match saved {
        Ok(()) => (StatusCode::CREATED, serde_json::json!({"data":true})),
        Err(None) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false})),
        Err(Some(reason)) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false,"message":refusal_text(&ctx,&reason)})),
    };
    Ok((status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], data.to_string()).into_response())
}

/// Recheck a committed slot without rewriting its credentials. Q binds the result to the captured digest.
async fn revalidate(app:&App,id:i64)->Result<Option<String>,WebError>{
    let inner=app.clone();
    let (credentials,version,values,kind)=app.db(move|c|{
        let (credentials,version,values)=crate::sync::validation_material(c,&inner.cipher,id).map_err(|_|WebError::Config("cannot read stored credential".into()))?;
        let kind=c.query_row("SELECT key_type FROM api_keys WHERE id=?1",[id],|r|r.get::<_,i64>(0))?;
        Ok((credentials,version,values,kind))
    }).await?;
    let handle=super::validator::ValidationHandle::stored(credentials,version);
    let result=handle.check(&app.alpaca_trading_url(handle.credentials()),kind).await?;
    let status=result.map(|result|match result {Validity::Correct=>1,Validity::Incorrect=>2,Validity::Pending(ref e)=>{
        app.settings_key_logger.warn(&format!("[Alpaca] API key validation failed: {}",scrub(&e.log_text(),&values)));0
    }});
    let jobs=app.job_wakers()?;let inner=app.clone();
    let result=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let fenced=crate::engine::model::check_credential_result(&tx,status.origin())?;
        store_status(&fenced,&status,id,inner.now()).map_err(|_|WebError::Config("cannot store credential status".into()))?;
        if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()))}
        tx.commit()?;inner.wake_engine();Ok(None)
    }).await;
    match result{
        Err(WebError::Engine(crate::engine::EngineError::CredentialsChanged))=>{jobs.wake(crate::sync::jobs::API_KEY_VALIDATOR,Some(&id.to_string()),None);Ok(None)},
        out=>out,
    }
}

#[derive(Template)]
#[template(path="settings/alpaca_form.html")]
struct AlpacaForm<'a> { v:&'a Ctx, csrf:&'a str, action:String, form_data:&'a str, connect_title:String, instructions_title:String, instructions:String, live:bool }
#[derive(Template)]
#[template(path="settings/alpaca_modal.html")]
struct AlpacaModal<'a> { card:&'a str }
#[derive(Template)]
#[template(path="settings/alpaca_full.html")]
struct AlpacaFull<'a> { v:&'a Ctx, card:&'a str, progress_title:String }
pub async fn form(axum::extract::State(app):axum::extract::State<App>, axum::extract::Extension(ctx):axum::extract::Extension<Ctx>) -> Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let exchange=tracker_exchange(&app,&ctx).await?;
    let Some(exchange)=exchange else {return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/pick_exchange/new")))};
    let requested=ctx.params.query("key_type").filter(|s|matches!(*s,"trading"|"read_only"));
    let kind=if requested==Some("trading"){0}else{2};
    let inner=app.clone();let owner=user.clone();
    let found=app.db(move|c| {
        let venue:Option<String>=c.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0)).optional()?;
        let healthy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 AND status=1)",[owner.id,exchange],|r|r.get(0))?;
        Ok((venue,healthy,Key::read(c,&inner,owner.id,exchange,kind)?,crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    let (venue,healthy,mut key,shell)=found;
    if venue.is_none(){return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/pick_exchange/new")))}
    if venue.as_deref().is_some_and(crate::web::bot::retired_type){ctx.session.lock().tracker_connect=Some(exchange);return Ok(retired_answer(&ctx))}
    if venue.as_deref()==Some("Exchanges::Alpaca"){ctx.session.lock().tracker_connect=Some(exchange);}
    if venue.as_deref()!=Some("Exchanges::Alpaca"){return Ok(crate::web::layout::not_ported_response(&ctx.method,&ctx.params.fullpath,ctx.turbo_frame.as_deref()))}
    if !(requested.is_none()&&healthy)&&key.status!=1&&key.key.as_deref().is_some_and(|s|!crate::ruby::blank(s))&&key.secret.as_deref().is_some_and(|s|!crate::ruby::blank(s)){
        if let Some(id)=key.id{
            if let Some(reason)=revalidate(&app,id).await?{
                let body=turbo::prepend_flash(&flash::render(&flash::take(&ctx.session,&[(flash::ALERT,refusal_text(&ctx,&reason))]))?);
                return Ok((StatusCode::UNPROCESSABLE_ENTITY,[(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],body).into_response())
            }
            let inner=app.clone();let user_id=user.id;
            key=app.db(move|c|Key::read(c,&inner,user_id,exchange,kind)).await?;
        }
    }
    if requested.is_none()&&(healthy||key.status==1){return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker")))}
    let csrf=ctx.csrf_token();
    let modal=ctx.turbo_frame.as_deref()==Some("modal");
    let instructions_key=if kind==2&&lookup(ctx.locale,"read_only_api.alpaca.instructions").is_some(){"read_only_api.alpaca.instructions"}else{"bot.api.alpaca.instructions"};
    let card=AlpacaForm{v:&ctx,csrf:&csrf,action:ctx.path(&format!("/tracker/add_api_key?exchange_id={exchange}&key_type={}",if kind==0{"trading"}else{"read_only"})),form_data:if modal{"data-controller=\"\" data-action=\"turbo:submit-end-&gt;modal--base#submitEnd\""}else{"data-controller=\"form--label-animations form--html5-validations\" data-turbo-frame=\"modal_content\""},connect_title:i18n::t(ctx.locale,"bot.setup.connect_to",&[("name",i18n::Arg::Text("Alpaca"))]),instructions_title:i18n::t(ctx.locale,"bot.setup.how_to_get_keys",&[("exchange",i18n::Arg::Text("Alpaca"))]),instructions:format!("<p>{}</p>",lookup(ctx.locale,instructions_key).unwrap_or("")),live:key.passphrase.as_deref()==Some("live")}.render()?;
    let body=if modal{AlpacaModal{card:&card}.render()?}else{AlpacaFull{v:&ctx,card:&card,progress_title:i18n::t(ctx.locale,"bot.setup.progress_steps.connect",&[])}.render()?};
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}

#[derive(Template)]
#[template(path="settings/pick_exchange.html")]
struct PickExchange<'a> { v:&'a Ctx, csrf:&'a str, progress_title:String, venues:Vec<(i64,&'static str)> }
/// Tracker::PickExchangesController#new: every available venue, stable ones first (Bots::Searchable), none disabled.
pub async fn pick_form(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let inner=app.clone();let owner=user.clone();
    let (venues,shell)=app.db(move|c|{
        let mut s=c.prepare("SELECT id,type FROM exchanges WHERE available=1 ORDER BY type IN ('Exchanges::Binance','Exchanges::BinanceUs','Exchanges::Coinbase','Exchanges::Kraken') DESC,name")?;
        let venues=s.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        let venues=venues.into_iter().map(|(id,class)|(id,crate::web::bot::exchange_wordmark(&crate::web::bot::Exchange{id,name:String::new(),class,maker_fee:None}.name_id()))).collect::<Vec<_>>();
        Ok((venues,crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    let csrf=ctx.csrf_token();
    let body=PickExchange{v:&ctx,csrf:&csrf,progress_title:i18n::t(ctx.locale,"bot.setup.progress_steps.exchange",&[]),venues}.render()?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}
/// Tracker::PickExchangesController#create: `Exchange.find` (any row), the session keeps only its id.
pub async fn pick_save(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    if ctx.user().is_none() {return Ok(auth::unauthenticated(&ctx))}
    // A missing or non-numeric id finds no row: RecordNotFound, Rails' 404.
    let Some(Ok(id))=ctx.params.form("exchange_id").or_else(||ctx.params.query("exchange_id")).map(str::parse::<i64>) else {return Ok(crate::web::layout::missing())};
    let found=app.db(move|c|Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get::<_,bool>(0))?)).await?;
    if !found {return Ok(crate::web::layout::missing())}
    ctx.session.lock().tracker_connect=Some(id);
    Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/add_api_key/new")))
}

async fn owned_bot_exchange(app:&App,ctx:&Ctx)->Result<Option<i64>,WebError>{
    let Some(user)=ctx.user() else{return Ok(None)};let owner=user.id;
    let id=ctx.params.route_path.split('/').nth(2).and_then(|s|s.parse::<i64>().ok());
    let Some(id)=id else{return Ok(None)};
    app.db(move|c|Ok(c.query_row("SELECT exchange_id FROM bots WHERE id=?1 AND user_id=?2",[id,owner],|r|r.get(0)).optional()?)).await
}
pub async fn bot_form(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    let Some(user)=ctx.user().cloned() else{return Ok(auth::unauthenticated(&ctx))};
    let Some(exchange)=owned_bot_exchange(&app,&ctx).await? else{return Ok(crate::web::layout::missing())};
    let inner=app.clone();let owner=user.clone();
    let (venue,key,shell)=app.db(move|c|Ok((c.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get::<_,String>(0))?,Key::read(c,&inner,owner.id,exchange,0)?,crate::web::shell::Shell::load(c,&inner,&owner)?))).await?;
    if venue!="Exchanges::Alpaca"{return Ok(crate::web::layout::not_ported_response(&ctx.method,&ctx.params.fullpath,ctx.turbo_frame.as_deref()))}
    let csrf=ctx.csrf_token();let action=ctx.path(ctx.params.route_path.trim_end_matches("/new"));
    let card=AlpacaForm{v:&ctx,csrf:&csrf,action,form_data:"data-controller=\"\" data-action=\"turbo:submit-end-&gt;modal--base#submitEnd\"",connect_title:i18n::t(ctx.locale,"bot.setup.connect_to",&[("name",i18n::Arg::Text("Alpaca"))]),instructions_title:i18n::t(ctx.locale,"bot.setup.how_to_get_keys",&[("exchange",i18n::Arg::Text("Alpaca"))]),instructions:format!("<p>{}</p>",lookup(ctx.locale,"bot.api.alpaca.instructions").unwrap_or("")),live:key.passphrase.as_deref()==Some("live")}.render()?;
    let body=AlpacaModal{card:&card}.render()?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}

pub fn store_status(c:&crate::engine::model::FencedTransaction<'_>,status:&crate::engine::model::Produced<i64>,id:i64,at:chrono::DateTime<chrono::Utc>)->Result<(),crate::sync::SyncError>{
    if !status.current_for(&c.producer_version()).is_fresh() || status.origin().as_ref().is_none_or(|producer|producer.id!=id){return Err(crate::sync::SyncError(crate::engine::model::CREDENTIALS_CHANGED.into()));}
    c.execute("UPDATE api_keys SET status=?1,updated_at=CASE WHEN status<>?1 THEN ?2 ELSE updated_at END WHERE id=?3",rusqlite::params![status.value,format_time(at),id])?;Ok(())
}

#[cfg(test)]
mod validation_producer_tests {
    use super::*;
    #[test]
    fn r4_validation_status_consumer_requires_bound_matching_reply(){
        use crate::engine::model::{self,Produced};
        let mut c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys(id INTEGER PRIMARY KEY,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT,status INTEGER,updated_at TEXT); INSERT INTO api_keys VALUES(1,'key-a','secret-a','paper',NULL,NULL,NULL,NULL,0,'2026-10-01');").unwrap();
        let current=model::credential_version_by_id(&c,1).unwrap().unwrap();
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        {
            let fenced=model::check_credential_result(&tx,&Some(current.clone())).unwrap();
            let unbound=Produced::new(1,None);
            assert!(store_status(&fenced,&unbound,1,chrono::Utc::now()).is_err(),"R4 status consumer rejects missing producer");
            let different=model::prepared_credential_version(1,&[Some("key-b".into()),Some("secret-b".into()),Some("paper".into()),None,None,None,None]);
            let foreign=Produced::new(1,Some(different));
            assert!(store_status(&fenced,&foreign,1,chrono::Utc::now()).is_err(),"R4 status consumer rejects another producer under the same key ID");
            assert_eq!(tx.query_row("SELECT status FROM api_keys WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),0,"refused results change no rows");
            let healthy=Produced::new(1,Some(current));
            assert!(store_status(&fenced,&healthy,1,chrono::Utc::now()).is_ok(),"R4 healthy status result publishes normally");
        }
        tx.commit().unwrap();
        assert_eq!(c.query_row("SELECT status FROM api_keys WHERE id=1",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}
