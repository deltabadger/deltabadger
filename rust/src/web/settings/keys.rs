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
use crate::identity::Account;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::sync::Arc;
use subtle::ConstantTimeEq;
pub trait Logger: Send + Sync {
    fn warn(&self, line: &str);
}
struct Stderr;
impl Logger for Stderr {
    fn warn(&self, line: &str) {
        crate::eline!("{line}");
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
/// The account's bots only (#710 §4.2).
fn settling(c: &Connection, account: &Account) -> Result<bool, WebError> {
    let mut stmt = c.prepare(&format!("SELECT b.id FROM bots b WHERE {} ORDER BY b.id", account.bots("b")))?;
    let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        let bot = crate::engine::model::load_bot(c, id)?;
        if bot.rust_placement().is_some() || !crate::engine::polling::waiting_ids(c, &bot)?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(crate) fn refusal_text(ctx: &Ctx, reason: &str) -> String {
    if reason == SETTLING { reason.to_string() }
    else { i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(reason))]) }
}
#[derive(Clone)]
pub(crate) struct Key {
    pub(crate) id: Option<i64>,
    pub(crate) key: Option<String>,
    pub(crate) secret: Option<String>,
    pub(crate) passphrase: Option<String>,
    pub(crate) status: i64,
    last_error: Option<String>,
    updated_at: Option<String>,
    extras: [Option<String>;4],
    realm: Option<String>,
    version: Option<crate::engine::model::CredentialVersion>,
    /// The account it was read from, where a save of it goes (#710).
    pub(crate) account: Account,
}
impl Key {
    pub(crate) fn read(
        c: &Connection,
        app: &App,
        account: Account,
        kind: i64,
    ) -> Result<Self, WebError> {
        if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=Self::read(&tx,app,account,kind)?;tx.commit()?;return Ok(out)}
        let row=c.query_row(&format!("SELECT id,key,secret,passphrase,status,last_sync_error,updated_at,access_token,rsa_signature_key,rsa_encryption_key,dh_param,ibkr_realm FROM api_keys k WHERE {} AND key_type=?1 ORDER BY id LIMIT 1",account.keys("k")),[kind],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?,[r.get::<_,Option<String>>(7)?,r.get(8)?,r.get(9)?,r.get(10)?],r.get::<_,Option<String>>(11)?))).optional()?;
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
                account,
            }),
            None => Ok(Self {
                id: None,
                key: None,
                secret: None,
                passphrase: None,
                status: 0,
                last_error: None,
                updated_at: None,
                extras: [None,None,None,None],realm:None,version:None,account,
            }),
        }
    }
    /// Settings::IbkrConnectionsController#activate's credentials over the stored key: the consumer key, the access token and
    /// its secret as submitted, the stored RSA keys and DH params re-passed, and a blank realm as `limited_poa`.
    pub(crate) fn with_ibkr_credentials(&self, consumer_key: &str, access_token: &str, secret: &str) -> Self {
        let mut candidate = self.clone();
        candidate.key = Some(consumer_key.into());
        candidate.secret = Some(secret.into());
        candidate.extras[0] = Some(access_token.into());
        if candidate.realm.as_deref().is_none_or(crate::v2::blank) { candidate.realm = Some(crate::venue::signers::ibkr::DEFAULT_REALM.into()); }
        candidate
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
use super::validator::Validity;
/// The prepared envelopes are the stored producer field for a newly validated slot.
/// The pending zero identity never enters any persisted account cache.
struct ValidatedKey {
    key:Key,
    columns:[Option<String>;7],
    handle:super::validator::ValidationHandle,
    result:crate::engine::model::Produced<Validity>,
    /// The key's account-identity read (crate::identity), recorded with the save under the same fence.
    identity:Option<crate::identity::Observed>,
}
async fn validate(app:&App,key:&Key,original:&Key,kind:i64,venue:&str)->Result<ValidatedKey,WebError>{
    let mut values=key.extras.iter().flatten().cloned().collect::<Vec<_>>();
    values.extend([original.key.as_ref(),original.secret.as_ref(),original.passphrase.as_ref()].into_iter().flatten().chain(original.extras.iter().flatten()).cloned());
    let columns=[key.key.as_ref(),key.secret.as_ref(),key.passphrase.as_ref(),key.extras[0].as_ref(),key.extras[1].as_ref(),key.extras[2].as_ref(),key.extras[3].as_ref()].map(|value|value.map(|value|app.cipher.encrypt(value)));
    let producer=crate::engine::model::prepared_credential_version(key.id.unwrap_or(0),&columns);
    // Session#initialize's reads, from the submitted fields over the stored ones.
    let ibkr=match (venue==crate::venue::ibkr::IBKR,&key.extras) {
        (true,[Some(access_token),Some(signature_key),Some(encryption_key),Some(dh_param)])=>Some(Box::new(crate::venue::signers::ibkr::Material{
            consumer_key:key.key.clone().unwrap_or_default(),access_token:access_token.clone(),access_token_secret:key.secret.clone().unwrap_or_default(),
            signature_key:signature_key.clone(),encryption_key:encryption_key.clone(),dh_param:dh_param.clone(),realm:key.realm.clone().unwrap_or_default()})),
        _=>None,
    };
    let handle=super::validator::ValidationHandle::stored(crate::crypto::Credentials{redaction_values:values,key:key.key.clone().unwrap_or_default(),secret:key.secret.clone().unwrap_or_default(),passphrase:key.passphrase.clone(),ibkr},producer);
    let result=handle.check_on(app,venue,kind).await?;
    let (result,identity)=result;
    app.observe_validation(result.origin().clone())?;
    Ok(ValidatedKey{key:key.clone(),columns,handle,result,identity})
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
    let bot_account=if bot_request {owned_bot_account(&app,&ctx).await?} else {None};
    if bot_request && bot_account.is_none(){return Ok(crate::web::layout::missing())}
    let exchange=if bot_request{bot_account.map(|a|a.exchange_id)}else{tracker_exchange(&app,&ctx).await?};
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
                .query_row("SELECT type,name FROM exchanges WHERE id=?1", [exchange], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default()))
                })
                .optional()?;
            // A bot's sheet saves into the bot's account; the tracker's into the exchange's (#710 slice 7: the one picked).
            let account = match bot_account { Some(account) => account, None => Account::first(c, id, exchange)? };
            let label = crate::identity::label_or(c, account.id, venue.as_ref().map_or("", |(_, name)| name.as_str()))?.plain();
            Ok((venue, label, Key::read(c, &inner, account, kind)?))
        })
        .await?;
    let (venue, label, original) = found;
    if !bot_request && venue.as_ref().is_some_and(|(class, _)| crate::web::bot::retired_type(class)) {
        ctx.session.lock().tracker_connect=Some(exchange);
        return Ok(retired_answer(&ctx));
    }
    if !ctx.params.form.iter().any(|(name, _)| name.starts_with("api_key[")) {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let Some((class, name)) = venue.filter(|(class, _)| ported(class)) else {
        return Ok(crate::web::layout::not_ported_for(&ctx));
    };
    if !bot_request {ctx.session.lock().tracker_connect=Some(exchange);}
    let live = ctx.params.form("api_key[passphrase]").map_or(original.passphrase.as_deref() == Some("live"), |mode| mode == "live");
    let outcome = submit(&app, &ctx, &class, &name, kind, original).await?;
    // D-TRACKER-SHEET, D-BOT-KEY-SHEET: a refused key says why under the sheet's fields.
    let message = match &outcome {
        Submitted::Correct => None,
        Submitted::Incorrect => Some(i18n::text(ctx.locale, "errors.incorrect_api_key_permissions", &[])),
        // ApplicationController#incorrect_api_key_message: the flags the venue named.
        Submitted::Permissions(missing, forbidden) => Some(super::key_check::permission_message(ctx.locale, &crate::tracker::rows::name_id(&class), &label, kind, missing, forbidden)),
        Submitted::Failed => Some(i18n::text(ctx.locale, "errors.api_key_permission_validation_failed", &[])),
        Submitted::Refused(reason) => Some(refusal_text(&ctx, reason)),
    };
    if let Some(message) = message {
        let (inner, v) = (app.clone(), ctx.clone());
        let (track, content) = app.db(move |c| if bot_request { Ok((String::new(), bot_step(c, &inner, &v, &class, &name, live, Some(&message))?)) }
                                              else { tracker_step(c, &inner, &v, exchange, &class, &name, kind, live, Some(&message)) }).await?;
        let body = turbo::stream("update", "modal_content", &format!("{}\n", crate::web::wizard::sheet_frame(&track, &content)?));
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)], format!("{body}\n")).into_response());
    }
    if bot_request {
        flash::set(&ctx.session,flash::NOTICE,ctx.t("errors.bots.api_key_success"));
        // 2.x's page refresh; the card's form closes the sheet on success (`modal--base#submitEnd`).
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

/// What a credential submission came to: ApiKey#validate_credentials!, then, for a correct key, its save.
pub(crate) enum Submitted { Correct, Incorrect, Permissions(Vec<String>, Vec<String>), Failed, Refused(String) }

/// The submitted `api_key[...]` fields over the stored key (`original`), validated on `venue` outside the write lock, and a
/// correct one saved (status correct, sync error cleared) inside it: rechecked against what was validated, guarded, then the
/// syncs a reading key has woken after the commit. An incorrect or unverified key is not saved. It is saved into the
/// account `original` was read from.
pub(crate) async fn submit(app: &App, ctx: &Ctx, venue: &str, venue_name: &str, kind: i64, original: Key) -> Result<Submitted, WebError> {
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
    submit_candidate(app,venue,venue_name,kind,original,candidate).await
}

/// `submit` of a candidate made by the caller over the stored key (`original`): the IBKR connect page's activation.
pub(crate) async fn submit_candidate(app: &App, venue: &str, venue_name: &str, kind: i64, original: Key, candidate: Key) -> Result<Submitted, WebError> {
    let validated=validate(app,&candidate,&original,kind,venue).await?;
    match &validated.result.value {
        Validity::Incorrect => return Ok(Submitted::Incorrect),
        Validity::Permissions { missing, forbidden } => {
            app.settings_key_logger.warn(&format!("[{venue_name}] API key validation: permissions do not match the steps"));
            return Ok(Submitted::Permissions(missing.clone(), forbidden.clone()));
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
                "[{venue_name}] API key validation failed: {}",
                scrub(&diagnostic.log_text(),&redaction_material)
            ));
            return Ok(Submitted::Failed);
        }
        Validity::Correct | Validity::Account(_) | Validity::PendingActivation => {}
    }
    // ApiKey#validate_credentials!: IBKR's activation wait is saved too (status pending_activation, its clock restarted).
    let pending=matches!(validated.result.value,Validity::PendingActivation);
    let account=match &validated.result.value {Validity::Account(account)=>Some(account.clone()),_=>None};
    let jobs=app.job_wakers()?;
    let inner = app.clone();
    let user_id=original.account.user_id;
    let outcome=app.db(move|c|{
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  // The account's row first: a key of the number 1 2.x saved resolves to it from here on.
  let target=original.account.ensure(&tx,inner.now())?;
  let current=Key::read(&tx,&inner,target,kind)?;
  if !original.same(&current){return Ok(Some("credential changed during validation".into()));}
  if !validated.result.current_for(&Some(validated.handle.producer().clone())).is_fresh(){return Ok(Some("credential changed during validation".into()));}
  let candidate=&validated.key;
  let now=format_time(inner.now());
  let status=if pending {3} else {1};
  // A submission awaiting activation always moves updated_at (activation_stalled? reads it as the last submission).
  let changed=pending||!(same(&candidate.key,&current.key)&&same(&candidate.secret,&current.secret)&&same(&candidate.passphrase,&current.passphrase)&&candidate.extras.iter().zip(&current.extras).all(|(a,b)|same(a,b))&&candidate.realm==current.realm&&current.status==1&&current.last_error.is_none());
  if current.id.is_some() && settling(&tx,&target)? { return Ok(Some(SETTLING.into())); }
  let id={
   let now=if changed {now} else {current.updated_at.clone().unwrap_or(now)};
   let key=validated.columns[0].clone();let secret=validated.columns[1].clone();let passphrase=validated.columns[2].clone();
   match current.id{
    Some(id)=>{tx.execute("UPDATE api_keys SET key=?1,secret=?2,passphrase=?3,status=?7,last_sync_error=NULL,updated_at=?4 WHERE id=?5 AND user_id=?6",rusqlite::params![key,secret,passphrase,&now,id,user_id,status])?;id},
    None=>{tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,?8,?7,?7)",rusqlite::params![user_id,target.exchange_id,kind,key,secret,passphrase,&now,status])?;
     let id=tx.last_insert_rowid();target.add_key(&tx,id)?;id}
   }
  };
  // #497: another credential may be another account, which syncs its own full history; its rows stay linked.
  if let Some(id)=current.id.filter(|_|!(same(&candidate.key,&current.key)&&same(&candidate.secret,&current.secret))){
   tx.execute("UPDATE api_keys SET last_synced_at=NULL WHERE id=?1",[id])?;
   tx.execute("DELETE FROM app_configs WHERE key IN (?1,?2,?3,?4)",(format!("rust_sync.ledger:{id}"),format!("rust_sync.ledger_splits:{id}"),format!("rust_sync.kraken_unresolved:{id}"),format!("rust_sync.kraken_repair_deferred:{id}")))?;
  }
  tx.execute("UPDATE api_keys SET access_token=?1,rsa_signature_key=?2,rsa_encryption_key=?3,dh_param=?4,ibkr_realm=?5 WHERE id=?6",rusqlite::params![validated.columns[3],validated.columns[4],validated.columns[5],validated.columns[6],candidate.realm,id])?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()));}
  // D-IBKR-ACCOUNT: the key binds to the account it names, or is refused (nothing saved) when bound to another.
  if let Some(account)=&account {
   if let Err(refusal)=crate::venue::ibkr_account::bind(&tx,&inner.cipher,id,account,inner.now()).map_err(WebError::Config)? {return Ok(Some(refusal));}
  }
  if let Some(observed)=&validated.identity{crate::identity::record(&tx,&inner.cipher,id,validated.handle.credentials(),validated.handle.producer(),observed,inner.now()).map_err(WebError::Config)?;}
  // The slots a restart would register (sync::jobs::register, tracker::jobs::register): a key that reads for its
  // venue gets its syncs, and its owner the tracker's jobs. Woken after the commit only.
  let ledger=crate::sync::ledger_reading_keys(&tx).map_err(|e|WebError::Config(e.0))?.contains(&id);
  let balances=crate::sync::balance_reading_keys(&tx).map_err(|e|WebError::Config(e.0))?.contains(&id);
  tx.commit()?;
  if kind==0 {inner.wake_engine();}
  if ledger {jobs.wake(crate::sync::jobs::LEDGER_SYNC,Some(&id.to_string()),None);}
  if balances {jobs.wake(crate::sync::jobs::BALANCE_SYNC,Some(&id.to_string()),None);}
  if ledger||balances {
   for job in [crate::tracker::jobs::TRACKER_LEDGER,crate::tracker::jobs::PORTFOLIO_BACKFILL] {jobs.wake(job,Some(&user_id.to_string()),None);}
  }
  Ok(None)
 }).await?;
    Ok(outcome.map_or(Submitted::Correct, Submitted::Refused))
}

#[derive(Template)]
#[template(path="settings/sync_warnings.html")]
struct Warnings{title:String,fixes:Vec<Fix>}
struct Fix{message:String,trading:bool,trading_path:String,reading_path:String,trading_label:String,add_label:String,replace_label:String}
/// `unresolved`: the key's last balance sync held codes no asset is named by (`rust_sync.kraken_unresolved:<key_id>`, D-KR-6),
/// a failure 2.x never records; it reads as the sync's own failure.
struct WarningKey{exchange:i64,account:Option<i64>,name:String,venue:String,status:i64,kind:i64,error:Option<String>,unresolved:bool}
impl WarningKey{
    fn failed(&self)->bool{self.unresolved||self.error.as_deref().is_some_and(|e|!crate::v2::blank(e))}
    /// ApiKey#missing_permission?: the venue's own permission_denied texts (Exchange#permission_error?), one table for every venue.
    fn permission(&self)->bool{crate::tracker::import::run::missing_permission(&self.venue,self.error.as_deref())}
}
/// `ApiKey.sync_warnings` rendered as tracker/_sync_warnings: the contents of #sync-warnings.
pub(crate) fn warnings(c:&Connection,v:&Ctx,user:i64)->Result<String,WebError>{
    let keys=crate::identity::with_key_account("k",|account|c.prepare(&format!("SELECT k.exchange_id,{},e.type,k.status,k.key_type,k.last_sync_error,EXISTS(SELECT 1 FROM app_configs a WHERE a.key='rust_sync.kraken_unresolved:'||k.id),{account} FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type!=1 ORDER BY k.id",crate::identity::plain_label_sql("e.name",account)))?
    .query_map([user],|r|Ok(WarningKey{exchange:r.get(0)?,account:r.get(7)?,name:r.get(1)?,venue:r.get(2)?,status:r.get(3)?,kind:r.get(4)?,error:r.get(5)?,unresolved:r.get(6)?}))?.collect::<Result<Vec<_>,_>>())?;
    // Per account (#710).
    let mut failed=vec![];let mut seen=std::collections::HashSet::new();
    for key in &keys{
        if !seen.insert((key.exchange,key.account)){continue}
        let group:Vec<_>=keys.iter().filter(|k|(k.exchange,k.account)==(key.exchange,key.account)).collect();
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

pub(crate) fn lookup(locale: &str, key: &str) -> Option<&'static str> {
    for language in [locale, i18n::DEFAULT] {
        let key = format!("{language}.{key}");
        if let Some((_, text)) = i18n::all().iter().find(|(name, _)| *name == key) {
            return Some(*text);
        }
    }
    None
}
/// settings/widgets/_api_keys for `user`.
pub fn list(c: &Connection, ctx: &Ctx, csrf: &str, user: i64) -> Result<String, WebError> {
    // Each key named by its account (#710 slice 6).
    let rows = crate::identity::with_key_account("k", |account| c.prepare(&format!("SELECT k.id,e.name,e.type,k.key_type,{} FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 ORDER BY k.id", crate::identity::label_number_sql(account)))?
        .query_map([user], |r| Ok((r.get::<_, i64>(0)?, crate::identity::Label { name: r.get(1)?, number: r.get(4)? }, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?
        .collect::<Result<Vec<_>, _>>())?;
    let keys: Vec<super::sections::Key> = rows
        .into_iter()
        .map(|(id, name, venue, kind)| {
            let name_id = crate::tracker::rows::name_id(&venue);
            let prefix = if kind == 1 { "withdrawal_api" } else { "bot.api" };
            let instructions = lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions")).is_some()
                || lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions.0.text_html")).is_some();
            super::sections::Key { id, name, name_id, read_only: kind == 2, withdrawal: kind == 1, instructions }
        })
        .collect();
    Ok(super::sections::api_keys(ctx, csrf, &keys))
}
/// settings/widgets/_market_data: the container's market data source, the one section Connect holds (D-BROKERS-AS-VENUES).
/// `show_coingecko_form`: 2.x's `@show_coingecko_form`, a switch to CoinGecko with no key to use. `platform_errors`: 2.x's
/// `@platform_errors` with `@show_platform_form`, a claim that failed. `wizard`: the provider whose connect card the index
/// wizard's Index view draws in its place (D-INDEX-CONNECT-IN-VIEW), its writes answering the wizard.
pub(crate) fn market_data(c:&Connection,app:&App,v:&Ctx,csrf:&str,show_coingecko_form:bool,platform_errors:Option<&str>,wizard:Option<&str>)->Result<String,WebError>{
    let setting=|name:&str|auth::app_config(c,&app.cipher,name);
    let has=|name:&str|Ok::<_,WebError>(setting(name)?.is_some_and(|s|!crate::v2::blank(&s)));
    let provider=setting("market_data_provider")?.unwrap_or_default();let hosted=app.config.market_data_url;
    let platform_connected=has("platform_connected_at")?;
    let proxies=super::platform::proxies(c)?;
    let hosted_note=|key:&str|i18n::t(v.locale,key,&[("provider_name",i18n::Arg::Text(&app.settings_market_provider_name))]);
    let coingecko_configured=!hosted&&provider=="coingecko";
    // What the reader is in the middle of outranks what is stored; on a plain visit that is what is stored.
    let selected=if platform_errors.is_some(){"deltabadger"}else if show_coingecko_form||coingecko_configured{"coingecko"}else if provider=="deltabadger"{"deltabadger"}else{""};
    let market_note=hosted_note("settings.market_data.deltabadger_configured");
    let platform_note=v.t(if proxies{"settings.platform.connected_with_proxies"}else{"settings.platform.connected_without_proxies"});
    let coingecko_instructions=v.t("setup.coingecko_api_key_hint_html");
    // D-MARKET-DATA-PICK: Settings' card: a claim refused or a CoinGecko switch with no key needs its form again; else the tile picked.
    let card=if platform_errors.is_some(){Some("deltabadger")}else if show_coingecko_form{Some("coingecko")}else{v.params.query("provider").filter(|p|matches!(*p,"coingecko"|"deltabadger"))};
    let m=super::sections::Market{hosted:hosted.then_some(market_note.as_str()),selected,coingecko_configured,platform_connected,platform_note:&platform_note,coingecko_instructions:&coingecko_instructions,platform_errors,card};
    Ok(match wizard { Some(provider) => super::sections::market_data_card(v,csrf,provider,&m,true), None => super::sections::market_data(v,csrf,&m) })
}
/// GET /settings/api_keys: the exchange keys.
pub async fn show_keys(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)); };
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let token = csrf.clone();
    let (pane, shell) = app.db(move |c| Ok((list(c, &v, &token, user.id)?, crate::web::shell::Shell::load(c, &inner, &user)?))).await?;
    let user = ctx.user().ok_or_else(|| WebError::Config("missing key owner".into()))?;
    super::view::section_page(&ctx, &csrf, user, &shell, "api_keys", &format!("  {pane}\n"), StatusCode::OK, vec![])
}
/// GET /settings/connect: for an admin, the container's market data; for anyone else an empty pane, as 2.x draws it.
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)); };
    let (inner, v, csrf) = (app.clone(), ctx.clone(), ctx.csrf_token());
    let token = csrf.clone();
    let (pane, shell) = app.db(move |c| {
        let pane = if user.admin { format!("\n{}\n", market_data(c, &inner, &v, &token, false, None, None)?) } else { String::new() };
        Ok((pane, crate::web::shell::Shell::load(c, &inner, &user)?))
    }).await?;
    let user = ctx.user().ok_or_else(|| WebError::Config("missing key owner".into()))?;
    super::view::section_page(&ctx, &csrf, user, &shell, "connect", &pane, StatusCode::OK, vec![])
}
/// GET /settings/api_key_permissions/:id: a key's walkthrough in the pane, the sidebar on API keys.
pub async fn modal(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let Some(id) = ctx.params.route_path.rsplit('/').next().and_then(|s| s.parse::<i64>().ok()) else {
        return Ok(crate::web::layout::missing());
    };
    let inner = app.clone();
    let v = ctx.clone();
    let found=app.db(move|c|{let key=crate::identity::with_key_account("k",|account|c.query_row(&format!("SELECT e.name,e.type,k.key_type,{} FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1 AND k.user_id=?2",crate::identity::plain_label_sql("e.name",account)),(id,user.id),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).optional())?;
        key.map(|(name,venue,kind,label)|Ok(((label,venue.clone()),key_instructions(c,&inner,&v,&venue,&name,kind)?,crate::web::shell::Shell::load(c,&inner,&user)?))).transpose()}).await?;
    let Some(((name, venue), instructions, shell)) = found else {
        return Ok(crate::web::layout::missing());
    };
    if !ported(&venue) {
        return Ok(crate::web::layout::not_ported_for(&ctx));
    }
    let csrf = ctx.csrf_token();
    let pane = format!(
        "  <a class=\"sheet__back\" href=\"{}\">\n    {}{}\n</a>  <h3 class=\"sheet__heading\">{}</h3>\n  <div class=\"sheet__prose\">{}</div>\n",
        i18n::escape(&ctx.path("/settings/api_keys")),
        super::dialog::BACK,
        ctx.t("settings.index.api_keys_section"),
        i18n::t(ctx.locale, "bot.setup.how_to_get_keys", &[("exchange", i18n::Arg::Text(&name))]),
        instructions
    );
    let user = ctx.user().ok_or_else(|| WebError::Config("missing key owner".into()))?;
    super::view::section_page(&ctx, &csrf, user, &shell, "api_keys", &pane, StatusCode::OK, vec![])
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
  remove(c,&inner,user.id,id,|tx|list(tx,&v,&csrf,user.id))
 }).await?;
    match outcome {
        None => Ok(crate::web::layout::missing()),
        Some(Ok(body)) => Ok((
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            turbo::stream("replace", "api_keys", &body),
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

/// A key's deletion (SettingsController#destroy_api_key, Settings::IbkrConnectionsController#destroy): its owner's working bots
/// on its account stopped first when it trades, its ledger rows kept (unlinked), its account identity and IBKR binding dropped.
/// `then` runs inside the transaction after the delete, before the commit. None when the user has no such key (or one whose
/// account resolves to none); refused with nothing written while a bot of the account settles an order, or when the
/// eligibility guard objects.
pub(crate) fn remove<T>(c: &Connection, inner: &App, user: i64, id: i64, then: impl FnOnce(&Connection) -> Result<T, WebError>) -> Result<Option<Result<T, String>>, WebError> {
  let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
  let kind:Option<i64>=tx.query_row("SELECT key_type FROM api_keys WHERE id=?1 AND user_id=?2",(id,user),|r|r.get(0)).optional()?;let Some(kind)=kind else{return Ok(None);};
  // Before the delete: the key's account is read through its row.
  let Some(account)=Account::of_key(&tx,id)? else{return Ok(None);};
  if settling(&tx,&account)? { return Ok(Some(Err(SETTLING.into()))); }
  let now=format_time(inner.now());
  if kind==0{
   let mut s=tx.prepare(&format!("SELECT b.id FROM bots b WHERE {} AND b.status IN(1,4,5,6) ORDER BY b.id",account.bots("b")))?;let ids=s.query_map([],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;drop(s);
   for bot in ids{tx.execute("UPDATE bots SET status=2,stopped_at=?1,stop_message_key=NULL,updated_at=?1 WHERE id=?2",(&now,bot))?;tx.execute("INSERT INTO bot_activity_logs(bot_id,event,level,message,details,created_at)VALUES(?1,'stopped',0,NULL,'{}',?2)",(bot,&now))?;}
  }
  tx.execute("UPDATE account_transactions SET api_key_id=NULL WHERE api_key_id=?1",[id])?;
  tx.execute("DELETE FROM api_keys WHERE id=?1 AND user_id=?2",(id,user))?;
  crate::identity::forget(&tx,id,&account)?;
  crate::venue::ibkr_account::forget(&tx,id)?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(Err(refusal.reason())));}
  let out=then(&tx)?;tx.commit()?;crate::venue::ibkr::forget(id);inner.wake_engine();Ok(Some(Ok(out)))
}

/// `legacy`'s answers to a pick (§4.2): an account not the user's live one of this exchange, and which account.
const MISSING: &str = "\0account not found";
const AMBIGUOUS: &str = "\0account required: ";
/// The browser's JSON endpoint: #497 replaces in place and resets only changed-key ledger progress.
pub async fn legacy(
    axum::extract::State(app): axum::extract::State<App>,
    axum::extract::Extension(ctx): axum::extract::Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    // 2.x reads `params.require(:api_key)` only: never a top-level `?secret=`. Its params merge the query string
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
    // #710 (PLAN-710-6 §4.1): the account the save names; none with one live account is the exchange's.
    // Only an omitted or null account_id is the exchange's account; one that is no id (`[]`) names none.
    let raw_account = ctx.params.json.as_ref().and_then(|j| j.get("api_key")).and_then(|root| root.get("account_id")).is_some_and(|v| !v.is_null());
    let posted_account = match parameter("account_id") {
        None if raw_account => return Ok(layout::missing()),
        None => None,
        Some(text) => match text.trim().parse::<i64>() { Ok(id) => Some(id), Err(_) => return Ok(layout::missing()) },
    };
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
  // The account's row first: a key of the number 1 2.x saved resolves to it from here on.
  let account=match crate::identity::pick(&tx,user.id,exchange,posted_account)?{
   crate::identity::Pick::One(account)=>account,
   crate::identity::Pick::NotFound=>return Ok(Err(Some(MISSING.into()))),
   crate::identity::Pick::Ambiguous(accounts)=>{
    let name:String=tx.query_row("SELECT coalesce(name,'') FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?;
    return Ok(Err(Some(format!("{AMBIGUOUS}{}",crate::identity::Pick::required(&name,&accounts)))));
   }
  }.ensure(&tx,inner.now())?;
  let current=Key::read(&tx,&inner,account,kind)?;
  if current.id.is_some() && settling(&tx,&account)? { return Ok(Err(Some(SETTLING.to_string()))); }
  let same_keys=same(&current.key,&key)&&same(&current.secret,&secret);
  if venue=="Exchanges::Hyperliquid"{
   let address=crate::v2::validation_regex(r"\A0x[0-9a-fA-F]{40}\z").map_err(|_|WebError::Config("invalid wallet pattern".into()))?;
   let agent=crate::v2::validation_regex(r"\A(0x)?[0-9a-fA-F]{64}\z").map_err(|_|WebError::Config("invalid agent pattern".into()))?;
   if !address.is_match(key.as_deref().unwrap_or(""))||!agent.is_match(secret.as_deref().unwrap_or("")){return Ok(Err(None));}
  }
  if !ported(&venue){return Ok(Err(Some("credential venue is not ported".into())));}
  let now=format_time(inner.now());
  let saved_passphrase=if same_keys {passphrase.as_ref().or(current.passphrase.as_ref())}else{passphrase.as_ref()};
  let id=if let Some(id)=current.id{
   let stamp=if !same_keys||current.status!=0 {now.clone()} else {current.updated_at.clone().unwrap_or_else(||now.clone())};
   if same_keys {
    tx.execute("UPDATE api_keys SET status=0,updated_at=?1 WHERE id=?2",(&stamp,id))?;
   } else {
    tx.execute("UPDATE api_keys SET key=?1,secret=?2,passphrase=?3,status=0,last_sync_error=NULL,last_synced_at=NULL,updated_at=?4 WHERE id=?5",(key.as_ref().map(|s|inner.cipher.encrypt(s)),secret.as_ref().map(|s|inner.cipher.encrypt(s)),saved_passphrase.map(|s|inner.cipher.encrypt(s)),stamp,id))?;
    tx.execute("DELETE FROM app_configs WHERE key IN (?1,?2,?3,?4)",(format!("rust_sync.ledger:{id}"),format!("rust_sync.ledger_splits:{id}"),format!("rust_sync.kraken_unresolved:{id}"),format!("rust_sync.kraken_repair_deferred:{id}")))?;
   }
   id
  }else{
   tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,0,?7,?7)",(user.id,exchange,kind,key.as_ref().map(|s|inner.cipher.encrypt(s)),secret.as_ref().map(|s|inner.cipher.encrypt(s)),passphrase.as_ref().map(|s|inner.cipher.encrypt(s)),&now))?;
   let id=tx.last_insert_rowid();account.add_key(&tx,id)?;id
  };
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Err(Some(refusal.reason())));}
  tx.commit()?;inner.wake_engine();jobs.wake(crate::sync::jobs::API_KEY_VALIDATOR,Some(&id.to_string()),None);if !same_keys{jobs.wake(crate::sync::jobs::LEDGER_SYNC,Some(&id.to_string()),None);}Ok(Ok(()))
 }).await?;
    if matches!(&saved,Err(Some(reason)) if reason=="credential venue is not ported"){return Ok(layout::not_ported_for(&ctx));}
    if matches!(&saved,Err(Some(reason)) if reason==MISSING){return Ok(layout::missing());}
    let (status, data) = match saved {
        Ok(()) => (StatusCode::CREATED, serde_json::json!({"data":true})),
        Err(None) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false})),
        // An exchange with 2+ live accounts and no account named: which one, in the services' words (§4.2).
        Err(Some(reason)) if reason.starts_with(AMBIGUOUS) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false,"message":&reason[AMBIGUOUS.len()..]})),
        Err(Some(reason)) => (StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"data":false,"message":refusal_text(&ctx,&reason)})),
    };
    Ok((status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], data.to_string()).into_response())
}

/// Recheck a committed slot without rewriting its credentials. Q binds the result to the captured digest.
pub(crate) async fn revalidate(app:&App,id:i64)->Result<Option<String>,WebError>{
    let inner=app.clone();
    let (credentials,version,values,kind)=app.db(move|c|{
        let (credentials,version,values)=crate::sync::validation_material(c,&inner.cipher,id).map_err(|_|WebError::Config("cannot read stored credential".into()))?;
        let (kind,venue,name)=c.query_row("SELECT k.key_type,e.type,e.name FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
        Ok((credentials,version,values,(kind,venue,name)))
    }).await?;
    let (kind,venue,name)=kind;
    let handle=super::validator::ValidationHandle::stored(credentials,version);
    let (result,identity)=handle.check_on(app,&venue,kind).await?;
    let account=match &result.value {Validity::Account(account)=>Some(account.clone()),_=>None};
    let status=result.map(|result|match result {Validity::Correct|Validity::Account(_)=>1,Validity::PendingActivation=>3,Validity::Incorrect=>2,Validity::Permissions{..}=>{
        app.settings_key_logger.warn(&format!("[{name}] API key validation: permissions do not match the steps"));2
    },Validity::Pending(ref e)=>{
        app.settings_key_logger.warn(&format!("[{name}] API key validation failed: {}",scrub(&e.log_text(),&values)));0
    }});
    // D-IBKR-INCONCLUSIVE (as the validator job): an IBKR check with no verdict leaves the key as it is.
    if venue==crate::venue::ibkr::IBKR && status.value==0 {return Ok(None)}
    let jobs=app.job_wakers()?;let inner=app.clone();
    let result=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let fenced=crate::engine::model::check_credential_result(&tx,status.origin())?;
        let before:i64=fenced.query_row("SELECT status FROM api_keys WHERE id=?1",[id],|r|r.get(0))?;
        let status=bound_status(&fenced,&inner.cipher,id,status.clone(),account.as_deref(),inner.now()).map_err(|e|WebError::Config(e.0))?;
        store_status(&fenced,&status,id,inner.now()).map_err(|_|WebError::Config("cannot store credential status".into()))?;
        if let Some(observed)=&identity{crate::identity::record(&fenced,&inner.cipher,id,handle.credentials(),handle.producer(),observed,inner.now()).map_err(WebError::Config)?;}
        if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()))}
        // The engine is woken by a status that moved; the same answer again changes nothing it reads.
        tx.commit()?;if before!=status.value{inner.wake_engine();}Ok(None)
    }).await;
    match result{
        Err(WebError::Engine(crate::engine::EngineError::CredentialsChanged))=>{jobs.wake(crate::sync::jobs::API_KEY_VALIDATOR,Some(&id.to_string()),None);Ok(None)},
        out=>out,
    }
}

#[derive(Template)]
#[template(path="settings/alpaca_form.html")]
struct AlpacaForm<'a> { v:&'a Ctx, csrf:&'a str, action:String, form_data:&'a str, connect_title:String, instructions_title:String, instructions:String, live:bool, error:Option<&'a str>,
    name_id:String, mark:&'static str, alpaca:bool, passphrase:bool, key_label:String, secret_label:String, back:Option<String> }

/// The venues whose keys this build checks (the wizard's list): every other venue's key route is a named refusal.
fn ported(class:&str)->bool{crate::web::wizard::keys::ported(class)}
/// BotHelper#render_api_key_instructions for a key of `kind` on venue `class` named `name`: the walkthrough, the IP its
/// keys are bound to (the venue's proxy, ExchangeProxy.for) in it; a reading key's own list, else the bot's.
pub(crate) fn key_instructions(c:&Connection,app:&App,ctx:&Ctx,class:&str,name:&str,kind:i64)->Result<String,WebError>{
    let name_id=crate::tracker::rows::name_id(class);
    let proxy=crate::venue::http::proxy_for(&name_id,crate::app_config::get(c,&app.cipher,&format!("proxy_{name_id}")).map_err(WebError::Config)?,&|k|std::env::var(k).ok());
    let ip=super::key_check::whitelist_ip_html(ctx.locale,proxy.as_deref());
    let html=|prefix:&str|super::key_check::instructions_html(ctx.locale,prefix,&name_id,name,&ip);
    let mut out=match kind{1=>html("withdrawal_api"),2=>html("read_only_api").or_else(||html("bot.api")),_=>html("bot.api")}.unwrap_or_default();
    // D-DESKTOP-STABLE-IP-NOTE: a desktop's own IP is what the key gets bound to when no proxy stands in front of the venue,
    // and a home connection's IP changes; a subscription's proxies don't. The link is a full-page navigation: the card is
    // drawn inside the wizard sheet's frame and inside Settings' pane.
    if ctx.desktop()&&proxy.is_none()&&auth::app_config(c,&app.cipher,"platform_connected_at")?.is_none_or(|at|crate::v2::blank(&at)){
        let url=ctx.path("/settings/connect?provider=deltabadger");
        out.push_str(&format!("<p class=\"key-note\">{}</p>",i18n::t(ctx.locale,"bot.api.desktop_stable_ip_html",&[("url",i18n::Arg::Text(&url))])));
    }
    Ok(out)
}
/// Exchange#requires_passphrase? outside Alpaca (whose passphrase is its Paper/Live switch): the API Passphrase row.
fn requires_passphrase(class:&str)->bool{class!=crate::sync::ALPACA&&crate::venue::registry::row(class).is_some_and(|r|r.passphrase)}
/// The venue's connect card as the sheet draws it (D-WIZARD-SHEET-KEY-FORM): Settings' fields and button (`.rinput`, `.rbutton`)
/// under the venue's name alone (the button says "Connect"), the walkthrough (`instructions`) below; a refused key's reason
/// (`error`) under the fields, which it marks invalid. `name`/`class` are the exchange row's name and type. With `back` (a
/// wizard's venue step) the venue's logo is Back (D-KEY-LOGO-BACK); without (the bot page's key, D-BOT-KEY-SHEET) it is a plain
/// header.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sheet_card(ctx:&Ctx,csrf:&str,action:String,form_data:&str,name:&str,class:&str,live:bool,instructions:String,error:Option<&str>,back:Option<String>)->Result<String,WebError>{
    let name_id=crate::tracker::rows::name_id(class);
    // BotHelper#api_key_field_label: the venue's own word for a field, else the generic one.
    let label=|field:&str|{let specific=format!("bot.api.{name_id}.{field}");if lookup(ctx.locale,&specific).is_some(){ctx.t(&specific)}else{ctx.t(&format!("bot.api.{field}_label"))}};
    Ok(AlpacaForm{v:ctx,csrf,action,form_data,connect_title:i18n::escape(name),
        instructions_title:i18n::t(ctx.locale,"bot.setup.how_to_get_keys",&[("exchange",i18n::Arg::Text(name))]),instructions,live,error,
        mark:crate::web::bot::exchange_svg(&name_id),alpaca:class=="Exchanges::Alpaca",passphrase:requires_passphrase(class),key_label:label("public_key"),secret_label:label("private_key"),name_id,back}.render()?)
}
pub async fn form(axum::extract::State(app):axum::extract::State<App>, axum::extract::Extension(ctx):axum::extract::Extension<Ctx>) -> Result<Response,WebError> {
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let exchange=tracker_exchange(&app,&ctx).await?;
    let Some(exchange)=exchange else {return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/pick_exchange/new")))};
    // D-TRACKER-IBKR-SHEET: IBKR's connection, whatever key type the link asked for, before any key is read the generic way
    // (its material may not decrypt; its own reader offers Disconnect then).
    let class:Option<String>=app.db(move|c|Ok(c.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0)).optional()?)).await?;
    if class.as_deref()==Some(crate::venue::ibkr::IBKR){
        ctx.session.lock().tracker_connect=Some(exchange);
        // What the connection's last write earned, drawn in the card: a frame does not show the layout's flash.
        let notice=std::mem::take(&mut ctx.session.lock().flash).pop().map(|(kind,text)|(kind==flash::ALERT,text));
        let (inner,v,owner)=(app.clone(),ctx.clone(),user.clone());
        let found=app.db(move|c|Ok((super::ibkr::tracker_step(c,&inner,&v,owner.id,notice)?,crate::web::shell::Shell::load(c,&inner,&owner)?))).await?;
        let (Some((track,content)),shell)=found else {return Ok(crate::web::layout::not_ported_for(&ctx))};
        let csrf=ctx.csrf_token();
        let body=crate::web::wizard::sheet_to(&ctx,"/tracker",&track,&content)?;
        return crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]});
    }
    let requested=ctx.params.query("key_type").filter(|s|matches!(*s,"trading"|"read_only"));
    let kind=if requested==Some("trading"){0}else{2};
    let inner=app.clone();let owner=user.clone();
    let found=app.db(move|c| {
        let venue:Option<(String,String)>=c.query_row("SELECT type,name FROM exchanges WHERE id=?1",[exchange],|r|Ok((r.get(0)?,r.get::<_,Option<String>>(1)?.unwrap_or_default()))).optional()?;
        let account=Account::first(c,owner.id,exchange)?;
        // healthy_trading_key: a correct bot key that lacks no permission the sync needs.
        let errors=c.prepare(&format!("SELECT last_sync_error FROM api_keys k WHERE {} AND key_type=0 AND status=1 ORDER BY id LIMIT 1",account.keys("k")))?.query_map([],|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;
        let healthy=venue.as_ref().is_some_and(|(class,_)|errors.iter().any(|error|!crate::tracker::import::run::missing_permission(class,error.as_deref())));
        Ok((venue,healthy,Key::read(c,&inner,account,kind)?,crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    let (venue,healthy,mut key,shell)=found;
    let Some((class,name))=venue else {return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/pick_exchange/new")))};
    if crate::web::bot::retired_type(&class){ctx.session.lock().tracker_connect=Some(exchange);return Ok(retired_answer(&ctx))}
    if !ported(&class){return Ok(crate::web::layout::not_ported_for(&ctx))}
    ctx.session.lock().tracker_connect=Some(exchange);
    if !(requested.is_none()&&healthy)&&key.status!=1&&key.key.as_deref().is_some_and(|s|!crate::v2::blank(s))&&key.secret.as_deref().is_some_and(|s|!crate::v2::blank(s)){
        if let Some(id)=key.id{
            if let Some(reason)=revalidate(&app,id).await?{
                let body=turbo::prepend_flash(&flash::render(&flash::take(&ctx.session,&[(flash::ALERT,refusal_text(&ctx,&reason))]))?);
                return Ok((StatusCode::UNPROCESSABLE_ENTITY,[(header::CONTENT_TYPE,turbo::CONTENT_TYPE)],body).into_response())
            }
            let (inner,account)=(app.clone(),key.account);
            key=app.db(move|c|Key::read(c,&inner,account,kind)).await?;
        }
    }
    if requested.is_none()&&(healthy||(key.status==1&&!crate::tracker::import::run::missing_permission(&class,key.last_error.as_deref()))){return Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker")))}
    let csrf=ctx.csrf_token();
    let (inner,v,live)=(app.clone(),ctx.clone(),key.passphrase.as_deref()==Some("live"));
    let (track,content)=app.db(move|c|tracker_step(c,&inner,&v,exchange,&class,&name,kind,live,None)).await?;
    let body=crate::web::wizard::sheet_to(&ctx,"/tracker",&track,&content)?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}

/// D-TRACKER-SHEET: the tracker's key step in the sheet, however it was asked for (the connect button's venue step, or a venue,
/// sync-warning or gap link straight to it): the track (the venue done, its link back to the venue step; Connect current) and
/// the venue's card, `error` under its fields.
#[allow(clippy::too_many_arguments)]
fn tracker_step(c:&Connection,app:&App,ctx:&Ctx,exchange:i64,class:&str,name:&str,kind:i64,live:bool,error:Option<&str>)->Result<(String,String),WebError>{
    use crate::web::wizard::{self, Stop};
    let instructions=key_instructions(c,app,ctx,class,name,kind)?;
    let action=ctx.path(&format!("/tracker/add_api_key?exchange_id={exchange}&key_type={}",if kind==0{"trading"}else{"read_only"}));
    let card=sheet_card(ctx,&ctx.csrf_token(),action,SHEET_FORM,name,class,live,instructions,error,Some(ctx.path("/tracker/pick_exchange/new")))?;
    let track=wizard::track(ctx,1,vec![Stop::new("exchange").name(Some(name.to_string())).back(ctx.path("/tracker/pick_exchange/new")),Stop::new("connect")])?;
    Ok((track,wizard::keys::key_pane(&card)?))
}

/// Tracker::PickExchangesController#new: every available venue, stable ones first (Bots::Searchable), none disabled, in the sheet
/// (D-TRACKER-SHEET): the pane's search box over the venues, brokers and exchanges apart; the track's Exchange current.
pub async fn pick_form(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    let Some(user)=ctx.user().cloned() else {return Ok(auth::unauthenticated(&ctx))};
    let inner=app.clone();let owner=user.clone();
    let (venues,connected,shell)=app.db(move|c|{
        let mut s=c.prepare("SELECT id,name,type FROM exchanges WHERE available=1")?;
        let venues=s.query_map([],|r|Ok(crate::web::bot::Exchange{id:r.get(0)?,name:r.get::<_,Option<String>>(1)?.unwrap_or_default(),class:r.get(2)?,maker_fee:None,number:None}))?.collect::<Result<Vec<_>,_>>()?;
        Ok((venues,crate::web::wizard::exchanges::connected(c,owner.id,&[0,2])?,crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    use crate::web::wizard::{self, exchanges, Stop};
    let csrf=ctx.csrf_token();
    let table=exchanges::Table{action:ctx.path("/tracker/pick_exchange"),csrf:csrf.clone(),param_name:"exchange_id".into(),venues:vec![],unavailable_note:String::new()};
    let content=exchanges::venue_pane(&ctx,table,venues,&connected)?;
    let body=wizard::sheet_to(&ctx,"/tracker",&wizard::track(&ctx,0,vec![Stop::new("exchange"),Stop::new("connect")])?,&content)?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}
/// Tracker::PickExchangesController#create: `Exchange.find` (any row), the session keeps only its id.
pub async fn pick_save(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    if ctx.user().is_none() {return Ok(auth::unauthenticated(&ctx))}
    // A missing or non-numeric id finds no row: RecordNotFound, 2.x's 404.
    let Some(Ok(id))=ctx.params.form("exchange_id").or_else(||ctx.params.query("exchange_id")).map(str::parse::<i64>) else {return Ok(crate::web::layout::missing())};
    let found=app.db(move|c|Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)",[id],|r|r.get::<_,bool>(0))?)).await?;
    if !found {return Ok(crate::web::layout::missing())}
    ctx.session.lock().tracker_connect=Some(id);
    Ok(crate::web::layout::redirect(StatusCode::FOUND,&ctx.path("/tracker/add_api_key/new")))
}

/// The account of the user's bot this request names (#710: its sheet reads and saves that account's key).
async fn owned_bot_account(app:&App,ctx:&Ctx)->Result<Option<Account>,WebError>{
    let Some(user)=ctx.user() else{return Ok(None)};let owner=user.id;
    let id=ctx.params.route_path.split('/').nth(2).and_then(|s|s.parse::<i64>().ok());
    let Some(id)=id else{return Ok(None)};
    app.db(move|c|{
        let exchange:Option<i64>=c.query_row("SELECT exchange_id FROM bots WHERE id=?1 AND user_id=?2",[id,owner],|r|r.get(0)).optional()?;
        Ok(match exchange {Some(_)=>Account::of_bot(c,id)?,None=>None})
    }).await
}
/// The form attributes of every card the sheet draws: answered in its frame (`modal_content`).
pub(crate) const SHEET_FORM:&str="data-controller=\"form--label-animations form--html5-validations\" data-turbo-frame=\"modal_content\"";
/// `/bots/:id`, from this request's `/bots/:id/add_api_key[/new]`.
fn bot_base(ctx:&Ctx)->String{ctx.params.route_path.trim_end_matches("/new").trim_end_matches("/add_api_key").to_string()}
/// D-BOT-KEY-SHEET: the bot page's key step ("Add new keys", the status bar's Connect) as the sheet's card with no track: the
/// venue's name a plain header (no venue step to go back to), `error` under its fields.
fn bot_step(c:&Connection,app:&App,ctx:&Ctx,class:&str,name:&str,live:bool,error:Option<&str>)->Result<String,WebError>{
    let instructions=key_instructions(c,app,ctx,class,name,0)?;
    let form_data=format!("{SHEET_FORM} data-action=\"turbo:submit-end-&gt;modal--base#submitEnd\"");
    let card=sheet_card(ctx,&ctx.csrf_token(),ctx.path(&format!("{}/add_api_key",bot_base(ctx))),&form_data,name,class,live,instructions,error,None)?;
    crate::web::wizard::keys::key_pane(&card)
}
pub async fn bot_form(axum::extract::State(app):axum::extract::State<App>,axum::extract::Extension(ctx):axum::extract::Extension<Ctx>)->Result<Response,WebError>{
    let Some(user)=ctx.user().cloned() else{return Ok(auth::unauthenticated(&ctx))};
    let Some(account)=owned_bot_account(&app,&ctx).await? else{return Ok(crate::web::layout::missing())};
    let inner=app.clone();let owner=user.clone();
    let v=ctx.clone();
    let (content,shell)=app.db(move|c|{
        let (class,name)=c.query_row("SELECT type,name FROM exchanges WHERE id=?1",[account.exchange_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?.unwrap_or_default())))?;
        if !ported(&class){return Ok((None,crate::web::shell::Shell::load(c,&inner,&owner)?))}
        let live=Key::read(c,&inner,account,0)?.passphrase.as_deref()==Some("live");
        Ok((Some(bot_step(c,&inner,&v,&class,&name,live,None)?),crate::web::shell::Shell::load(c,&inner,&owner)?))
    }).await?;
    let Some(content)=content else{return Ok(crate::web::layout::not_ported_for(&ctx))};
    let csrf=ctx.csrf_token();
    let body=crate::web::wizard::sheet_to(&ctx,&bot_base(&ctx),"",&content)?;
    crate::web::shell::application(&ctx,&csrf,&user,&shell,crate::web::layout::Page{status:StatusCode::OK,body,flash_now:vec![]})
}

/// A correct IBKR check's status as stored: the key is bound to the account the check named, or it is incorrect when it is
/// bound to another (D-IBKR-ACCOUNT; nothing else is written). Any other venue's status as it is.
pub fn bound_status(c:&Connection,cipher:&crate::crypto::Cipher,id:i64,status:crate::engine::model::Produced<i64>,account:Option<&str>,at:chrono::DateTime<chrono::Utc>)->Result<crate::engine::model::Produced<i64>,crate::sync::SyncError>{
    let Some(account)=account else {return Ok(status)};
    match crate::venue::ibkr_account::bind(c,cipher,id,account,at).map_err(crate::sync::SyncError)? {
        Ok(())=>Ok(status),
        Err(refusal)=>{crate::engine::log(&format!("[Interactive Brokers] API key validation: {refusal}"));Ok(status.map(|_|2))}
    }
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
