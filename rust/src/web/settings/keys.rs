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
pub const PAPER_ONLY:&str="This build accepts paper keys only.";
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
    if reason == SETTLING || reason == PAPER_ONLY { reason.to_string() }
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
async fn validate(app:&App,key:&Key,original:&Key,kind:i64)->Result<Validity,WebError>{
    let mut values=key.extras.iter().flatten().cloned().collect::<Vec<_>>();
    values.extend([original.key.as_ref(),original.secret.as_ref(),original.passphrase.as_ref()].into_iter().flatten().chain(original.extras.iter().flatten()).cloned());
    super::validator::check(&app.settings_key_url,&crate::crypto::Credentials{ redaction_values:values,key:key.key.clone().unwrap_or_default(),secret:key.secret.clone().unwrap_or_default(),passphrase:key.passphrase.clone()},kind).await
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
    if let Some(id)=found{ctx.session.lock().tracker_connect=Some(id);}
    Ok(found)
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
    if !ctx
        .params
        .form
        .iter()
        .any(|(name, _)| name.starts_with("api_key["))
    {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
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
    if venue.is_some()&&!bot_request{ctx.session.lock().tracker_connect=Some(exchange);}
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
    if candidate.passphrase.as_deref() == Some("live") {
        let body = turbo::prepend_flash(&flash::render(&flash::take(
            &ctx.session,
            &[(flash::ALERT, PAPER_ONLY.into())],
        ))?);
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            body,
        )
            .into_response());
    }
    match validate(&app, &candidate, &original, kind).await? {
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
  let now=format_time(inner.now());
  let changed=!(same(&candidate.key,&current.key)&&same(&candidate.secret,&current.secret)&&same(&candidate.passphrase,&current.passphrase)&&candidate.extras.iter().zip(&current.extras).all(|(a,b)|same(a,b))&&candidate.realm==current.realm&&current.status==1&&current.last_error.is_none());
  if current.id.is_some() && settling(&tx,user.id,exchange)? { return Ok(Some(SETTLING.into())); }
  {
   let now=if changed {now} else {current.updated_at.clone().unwrap_or(now)};
   let key=candidate.key.as_ref().map(|s|inner.cipher.encrypt(s));let secret=candidate.secret.as_ref().map(|s|inner.cipher.encrypt(s));let passphrase=candidate.passphrase.as_ref().map(|s|inner.cipher.encrypt(s));
   match current.id{
    Some(id)=>{tx.execute("UPDATE api_keys SET key=?1,secret=?2,passphrase=?3,status=1,last_sync_error=NULL,updated_at=?4 WHERE id=?5 AND user_id=?6",(key,secret,passphrase,&now,id,user.id))?;},
    None=>{tx.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,key,secret,passphrase,status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7)",(user.id,exchange,kind,key,secret,passphrase,&now))?;}
   }
  }
  tx.execute("UPDATE api_keys SET access_token=?1,rsa_signature_key=?2,rsa_encryption_key=?3,dh_param=?4,ibkr_realm=?5 WHERE user_id=?6 AND exchange_id=?7 AND key_type=?8",rusqlite::params![candidate.extras[0].as_ref().map(|v|inner.cipher.encrypt(v)),candidate.extras[1].as_ref().map(|v|inner.cipher.encrypt(v)),candidate.extras[2].as_ref().map(|v|inner.cipher.encrypt(v)),candidate.extras[3].as_ref().map(|v|inner.cipher.encrypt(v)),candidate.realm,user.id,exchange,kind])?;
  if let Err(refusal)=eligibility::guard(&tx,&inner.cipher,None){return Ok(Some(refusal.reason()));}
  let id:i64=tx.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=?3",(user.id,exchange,kind),|r|r.get(0))?;
  tx.commit()?;
  inner.wake_engine();
  jobs.wake(crate::sync::jobs::LEDGER_SYNC,Some(&id.to_string()),None);
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
fn warnings(c:&Connection,v:&Ctx,user:i64)->Result<String,WebError>{
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
            fixes.push(Fix{message:label(&format!("tracker.key_fix.{}.{reason}",if key.kind==0{"trading"}else{"read_only"})),trading:key.kind==0,trading_path:v.path(&format!("/tracker/add_api_key/new?exchange_id={}&key_type=trading",key.exchange)),reading_path:v.path(&format!("/tracker/add_api_key/new?exchange_id={}&key_type=read_only",key.exchange)),trading_label:label("tracker.replace_trading_key"),add_label:label("tracker.add_tracker_key"),replace_label:label("tracker.replace_tracker_key")});
        }
    }
    Ok(Warnings{title:i18n::t(v.locale,"tracker.sync_failed_for",&[("exchanges",i18n::Arg::Text(&names))]),fixes}.render()?+"\n")
}

#[derive(askama::Template)]
#[template(path = "settings/api_keys.html")]
struct KeysView<'a> {
    v: &'a Ctx,
    trading: Vec<ListKey>,
    withdrawal: Vec<ListKey>,
}
struct ListKey {
    id: i64,
    name: String,
    instructions: bool,
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
pub fn list(c: &Connection, ctx: &Ctx, user: i64) -> Result<String, WebError> {
    let mut s=c.prepare("SELECT k.id,e.name,e.type,k.key_type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 ORDER BY k.id")?;
    let rows = s
        .query_map([user], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut trading = vec![];
    let mut withdrawal = vec![];
    for (id, name, venue, kind) in rows {
        let name_id = venue
            .strip_prefix("Exchanges::")
            .unwrap_or("")
            .to_lowercase();
        let prefix = if kind == 1 {
            "withdrawal_api"
        } else {
            "bot.api"
        };
        let instructions = lookup(ctx.locale, &format!("{prefix}.{name_id}.instructions"))
            .is_some()
            || lookup(
                ctx.locale,
                &format!("{prefix}.{name_id}.instructions.0.text_html"),
            )
            .is_some();
        let key = ListKey {
            id,
            name,
            instructions,
        };
        if kind == 1 {
            withdrawal.push(key)
        } else {
            trading.push(key)
        }
    }
    Ok(KeysView {
        v: ctx,
        trading,
        withdrawal,
    }
    .render()?)
}
#[derive(askama::Template)]
#[template(path = "settings/connect.html")]
struct Connect<'a> {v:&'a Ctx,admin:bool,keys:String,stocks:String,market_data:String}
#[derive(Template)]
#[template(path="settings/stocks.html")]
struct Stocks<'a>{v:&'a Ctx,csrf:&'a str,configured:bool,catalog_active:bool,paper:bool,mode:String,hosted:bool,hosted_note:String,configured_note:String,connect_alpaca:String,instructions_alpaca:String,instructions_alpaca_body:String}
#[derive(Template)]
#[template(path="settings/market_data.html")]
struct MarketData<'a>{v:&'a Ctx,csrf:&'a str,configured:bool,selected:String,coingecko:bool,deltabadger:bool,has_coingecko_key:bool,hosted:bool,hosted_note:String,platform_connected:bool,platform_note:String,connect_coingecko:String,connect_platform:String,instructions_coingecko:String}
fn connection_widgets(c:&Connection,app:&App,v:&Ctx,csrf:&str)->Result<(String,String),WebError>{
    let setting=|name:&str|auth::app_config(c,&app.cipher,name);
    let has=|name:&str|Ok::<_,WebError>(setting(name)?.is_some_and(|s|!crate::ruby::blank(&s)));
    let provider=setting("market_data_provider")?.unwrap_or_default();let hosted=app.config.market_data_url;
    let deltabadger=hosted||provider=="deltabadger";let coingecko=!hosted&&provider=="coingecko";
    let configured=if !hosted&&deltabadger{has("market_data_url")?&&has("market_data_token")?}else{hosted||!crate::ruby::blank(&provider)};
    let platform_connected=has("platform_connected_at")?;
    let proxies:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE key LIKE 'proxy_%')",[],|r|r.get(0))?;
    let connect=|name:&str|i18n::t(v.locale,"bot.setup.connect_to",&[("name",i18n::Arg::Text(name))]);
    let instruction=|name:&str|i18n::t(v.locale,"bot.setup.how_to_get_keys",&[("exchange",i18n::Arg::Text(name))]);
    let hosted_note=|key:&str|i18n::t(v.locale,key,&[("provider_name",i18n::Arg::Text(&app.settings_market_provider_name))]);
    let market=MarketData{v,csrf,configured,selected:if hosted{"deltabadger".into()}else{provider},coingecko,deltabadger,has_coingecko_key:has("coingecko_api_key")?,hosted,hosted_note:hosted_note("settings.market_data.deltabadger_configured"),platform_connected,platform_note:v.t(if proxies{"settings.platform.connected_with_proxies"}else{"settings.platform.connected_without_proxies"}),connect_coingecko:connect("CoinGecko"),connect_platform:connect(&v.t("settings.platform.option")),instructions_coingecko:instruction("CoinGecko")}.render()?;
    let catalog_active=deltabadger||c.query_row("SELECT EXISTS(SELECT 1 FROM tickers t JOIN exchanges e ON e.id=t.exchange_id WHERE t.available=1 AND e.type IN('Exchanges::Alpaca','Exchanges::Ibkr'))",[],|r|r.get::<_,bool>(0))?;
    let stored_mode=setting("alpaca_mode")?;let mode=if stored_mode.as_deref()==Some("paper"){"paper"}else{"live"};
    let display_mode=stored_mode.map(|mode|{let mut chars=mode.chars();chars.next().map(|ch|ch.to_uppercase().collect::<String>()+&chars.as_str().to_lowercase()).unwrap_or_default()}).unwrap_or_else(||"Paper".into());
    let stocks=Stocks{v,csrf,configured:has("alpaca_api_key")?&&has("alpaca_api_secret")?,catalog_active,paper:mode=="paper",mode:mode.into(),hosted:deltabadger,hosted_note:hosted_note("settings.stocks.deltabadger_configured"),configured_note:i18n::t(v.locale,"settings.stocks.configured",&[("mode",i18n::Arg::Text(&display_mode))]),connect_alpaca:connect("Alpaca"),instructions_alpaca:instruction("Alpaca"),instructions_alpaca_body:format!("<p>{}</p>",lookup(v.locale,"bot.api.alpaca.instructions").unwrap_or(""))}.render()?;
    Ok((stocks,market))
}
pub async fn show(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let inner=app.clone();let v=ctx.clone();let csrf=ctx.csrf_token();let token=csrf.clone();
    let (keys,shell,stocks,market_data)=app.db(move|c|{
        let (stocks,market_data)=connection_widgets(c,&inner,&v,&token)?;
        Ok((list(c,&v,user.id)?,crate::web::shell::Shell::load(c,&inner,&user)?,stocks,market_data))
    }).await?;
    let user=ctx.user().ok_or_else(||WebError::Config("missing key owner".into()))?;
    let body=Connect{v:&ctx,admin:user.admin,keys,stocks,market_data}.render()?;
    crate::web::shell::application(
        &ctx,
        &csrf,
        user,
        &shell,
        crate::web::layout::Page {
            status: StatusCode::OK,
            body,
            flash_now: vec![],
        },
    )
}
#[derive(askama::Template)]
#[template(path = "settings/delete_key.html")]
struct DeleteKey<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    id: i64,
    name: String,
}
#[derive(askama::Template)]
#[template(path = "settings/key_permissions.html")]
struct Permissions {
    instructions_alpaca: String,
    instructions: String,
}
pub async fn modal(app: App, ctx: Ctx) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let id = ctx
        .params
        .route_path
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok());
    let Some(id) = id else {
        return Ok(crate::web::layout::missing());
    };
    let inner = app.clone();
    let found=app.db(move|c|{let key=c.query_row("SELECT e.name,e.type,k.key_type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1 AND k.user_id=?2",(id,user.id),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).optional()?;key.map(|k|Ok((k,crate::web::shell::Shell::load(c,&inner,&user)?))).transpose()}).await?;
    let Some(((name, venue, _kind), shell)) = found else {
        return Ok(crate::web::layout::missing());
    };
    let csrf = ctx.csrf_token();
    let body = if ctx
        .params
        .route_path
        .starts_with("/settings/confirm_destroy_api_key/")
    {
        DeleteKey {
            v: &ctx,
            csrf: &csrf,
            id,
            name,
        }
        .render()?
    } else {
        if venue != "Exchanges::Alpaca" {
            return Ok(crate::web::layout::not_ported_response(
                &ctx.method,
                &ctx.params.fullpath,
                ctx.turbo_frame.as_deref(),
            ));
        }
        Permissions {
            instructions_alpaca: i18n::t(
                ctx.locale,
                "bot.setup.how_to_get_keys",
                &[("exchange", i18n::Arg::Text(&name))],
            ),
            instructions: format!(
                "<p>{}</p>",
                lookup(ctx.locale, "bot.api.alpaca.instructions").unwrap_or("")
            ),
        }
        .render()?
    };
    crate::web::shell::application(
        &ctx,
        &csrf,
        ctx.user()
            .ok_or_else(|| WebError::Config("missing key owner".into()))?,
        &shell,
        crate::web::layout::Page {
            status: StatusCode::OK,
            body,
            flash_now: vec![],
        },
    )
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
  let body=list(&tx,&v,user.id)?;tx.commit()?;inner.wake_engine();Ok(Some(Ok(body)))
 }).await?;
    match outcome {
        None => Ok(crate::web::layout::missing()),
        Some(Ok(body)) => Ok((
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            body + "\n",
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

/// The browser's JSON endpoint: #497 replaces in place and resets only changed-key ledger progress.
pub async fn legacy(
    axum::extract::State(app): axum::extract::State<App>,
    axum::extract::Extension(ctx): axum::extract::Extension<Ctx>,
) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let parameter = |name: &str| {
        ctx.params
            .query(name)
            .or_else(|| ctx.params.form(&format!("api_key[{name}]")))
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
  if venue=="Exchanges::Alpaca"&&passphrase.as_deref().or(current.passphrase.as_deref())==Some("live"){return Ok(Err(Some(PAPER_ONLY.into())));}
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
    if credentials.passphrase.as_deref()==Some("live"){return Ok(None)}
    let result=super::validator::check(&app.settings_key_url,&credentials,kind).await?;
    let status=match result {Validity::Correct=>1,Validity::Incorrect=>2,Validity::Pending(ref e)=>{
        app.settings_key_logger.warn(&format!("[Alpaca] API key validation failed: {}",scrub(&e.log_text(),&values)));0
    }};
    let jobs=app.job_wakers()?;let inner=app.clone();
    let result=app.db(move|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let fenced=crate::engine::model::check_credential_result(&tx,&Some(version))?;
        store_status(&fenced,status,id,inner.now()).map_err(|_|WebError::Config("cannot store credential status".into()))?;
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
    ctx.session.lock().tracker_connect=Some(exchange);
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

pub fn store_status(c:&crate::engine::model::FencedTransaction<'_>,status:i64,id:i64,at:chrono::DateTime<chrono::Utc>)->Result<(),crate::sync::SyncError>{
    c.execute("UPDATE api_keys SET status=?1,updated_at=CASE WHEN status<>?1 THEN ?2 ELSE updated_at END WHERE id=?3",rusqlite::params![status,format_time(at),id])?;Ok(())
}
