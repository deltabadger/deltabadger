//! Current fiat rates. Fetch outside the database transaction; recheck configuration before saving.
use crate::{figures::{self,at::At,db::Ticker,dec::Dec,market::{Failure,Fetch,MarketData,Member,Quoted,Venue},num::{Num,NumError},page_market::{FxStore,HeldRate},totals::Denomination},jobs::data_api::{Config,DataApi},web::{App,WebError}};
use std::sync::Arc;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

pub const SUCCESS_TTL:i64=12*60*60;
/// A failed conversion is held this long before it is asked again (Denomination::FAILURE_BACKOFF). Rails'
/// Utilities::Currency holds no failure; the figures fill passes several times per load, and every cut.
pub const FAILURE_TTL:i64=5*60;
/// The install's held conversions (`figures::page_market::FxStore`), shared by the tracker and the figures, and one
/// fetch at a time: concurrent requests share a fill, and none holds App::db while it waits on the network.
#[derive(Default)]
pub struct Cache { pub store:Arc<FxStore>, fetching:tokio::sync::Mutex<()> }
#[derive(Clone)]
pub struct Prepared {identity:Option<String>,requested:String,rate:Option<String>,until:i64}
fn unavailable()->WebError {WebError::Config("Currency conversion unavailable".into())}
/// The provider's answers one fill fetched, as Utilities::Currency reads them: the exchange-rate table (MarketData.
/// get_exchange_rates), and coins' prices (MarketData.get_price). A request not answered is noted (`asked`).
#[derive(Default)]
struct Raw { rates:Option<Result<Value,()>>, prices:BTreeMap<(String,String),Result<Value,()>>, asked:std::cell::RefCell<(bool,std::collections::BTreeSet<(String,String)>)> }
impl MarketData for Raw {
    fn prices(&self,_:&Venue,_:&[String])->Fetch<Vec<(String,Member<Dec>)>> {Err(Failure::Failed("Market data unavailable".into()))}
    fn candles(&self,_:&Venue,_:&Ticker,_:At,_:i64,_:bool)->Fetch<Vec<(At,Dec)>> {Err(Failure::Failed("Market data unavailable".into()))}
    fn coin_price(&self,coin:&str,currency:&str)->Fetch<Quoted> {
        let key=(coin.to_string(),currency.to_string());
        let Some(answer)=self.prices.get(&key) else { self.asked.borrow_mut().1.insert(key); return Err(Failure::Failed("Currency conversion unavailable".into())) };
        let Ok(body)=answer else { return Err(Failure::Failed("Currency conversion unavailable".into())) };
        // `result.data.dig('data', coin_id, currency)`, as the provider's JSON carried it; nil is no price.
        match Quoted::from_json(&body["data"][coin][currency])? { Some(price)=>Ok(price), None=>Err(Failure::Failed(format!("Price not found for {coin} in {currency}"))) }
    }
    fn exchange_rates(&self)->Fetch<Vec<(String,Member<Num>)>> {
        let Some(answer)=&self.rates else { self.asked.borrow_mut().0=true; return Err(Failure::Failed("Currency conversion unavailable".into())) };
        let Some(rates)=answer.as_ref().ok().and_then(Value::as_object) else{return Err(Failure::Failed("Currency conversion unavailable".into()))};
        let mut out=vec![];
        for(currency,rate) in rates {
            let value=&rate["value"];
            if value.is_null(){continue;}
            let n=match Num::from_fx_json(value){Ok(Some(n))=>Ok(n),Ok(None)=>Err(NumError::NotANumber),Err(e)=>Err(e)};
            out.push((currency.clone(),n));
        }
        Ok(out)
    }
}
/// The configured market-data provider: its fingerprint ("none" without one), and how to reach it.
pub fn provider(c:&Connection)->Result<(String,Option<Config>),WebError> {
    let identity=crate::engine::provider::fingerprint(c).map_err(|_|unavailable())?.unwrap_or_else(||"none".into());
    Ok((identity,crate::engine::provider::config(c).map_err(|_|unavailable())?))
}
/// Holds an answer for every conversion asked (from, to) that is not held at `now` for this provider: each is computed as
/// Utilities::Currency.exchange_rate computes it, from what the provider answers, and held SUCCESS_TTL or six minutes
/// (`totals::held_for`) when it is a rate, FAILURE_TTL when it is not. No provider: each is held as a failure, and the
/// network is never asked. Called outside App::db and outside any page request: by the figures fill and the tracker.
pub async fn fill(app:&App,identity:&str,config:Option<Config>,pairs:Vec<(String,String)>)->Result<(),WebError>{
    let _one=app.fx_cache.fetching.lock().await;
    let now=app.now().timestamp();
    let store=app.fx_cache.store.clone();
    let pairs:Vec<(String,String)>=pairs.into_iter().filter(|(from,to)|store.get(identity,from,to,now).is_none()).collect();
    if pairs.is_empty(){return Ok(());}
    // What the provider must be asked: computing each conversion against no answers names them.
    let asking=pairs.clone();
    let (rates,coins)=app.db(move|c|{
        let raw=Raw::default();
        for (from,to) in &asking { let _=figures::totals::conversion(c,&raw,from,to); } // allow-swallow: only what it asked is read
        let asked=raw.asked.into_inner();
        Ok((asked.0,asked.1))
    }).await?;
    let mut raw=Raw::default();
    if let Some(config)=config {
        if matches!(app.figure_source,crate::web::figure::loading::Source::Script(_)|crate::web::figure::loading::Source::Counted(..)) {
            let credentials=crate::crypto::Credentials{ redaction_values:vec![],key:String::new(),secret:String::new(),passphrase:None};
            let wire=||crate::web::figure::loading::Wire::new(&credentials,app.figure_source.clone());
            ask(&DataApi::new(config,wire(),wire()),rates,&coins,&mut raw).await;
        } else { ask(&DataApi::live(config),rates,&coins,&mut raw).await; }
    } else {
        if rates { raw.rates=Some(Err(())); }
        for coin in coins { raw.prices.insert(coin,Err(())); }
    }
    let identity=identity.to_string();
    app.db(move|c|{
        for (from,to) in &pairs {
            let held=match figures::totals::conversion(c,&raw,from,to) {
                Ok(Ok(rate))=>HeldRate::of(&rate).map(|rate|(rate,figures::totals::held_for(from,to))),
                Ok(Err(failure))=>Err(failure),
                Err(_)=>Err(Failure::NotComputed("Currency conversion unavailable".into())),
            };
            match held {
                Ok((rate,ttl))=>store.put(&identity,from,to,Ok(rate),now.saturating_add(ttl),now),
                Err(failure)=>store.put(&identity,from,to,Err(failure),now.saturating_add(FAILURE_TTL),now),
            }
        }
        Ok(())
    }).await
}
async fn ask<T:crate::venue::http::Transport>(api:&DataApi<T>,rates:bool,coins:&std::collections::BTreeSet<(String,String)>,raw:&mut Raw) {
    if rates { raw.rates=Some(api.exchange_rates().await.map_err(|_|())); }
    for (coin,currency) in coins {
        let answer=api.get("/api/v1/prices",&[("coin_ids",coin),("vs_currencies",currency)],false).await.map_err(|_|());
        raw.prices.insert((coin.clone(),currency.clone()),answer);
    }
}
impl Prepared {
    /// The same source and requested currency must still apply inside the guarded transaction.
    pub fn denomination(&self,c:&Connection,owner:i64,now:i64)->Result<Option<Denomination>,WebError>{
        let current:String=c.query_row("SELECT display_currency FROM users WHERE id=?1",[owner],|r|r.get(0))?;
        let identity=crate::engine::provider::fingerprint(c).map_err(|_|unavailable())?;
        if now>=self.until{return Ok(None);}
        if figures::totals::normalized_currency(&current)!=self.requested || identity!=self.identity{return Ok(None);}
        self.rate.as_ref().map(|rate| {
            let rate=Dec::strict(rate).map_err(|_|unavailable())?;
            if !rate.is_positive(){return Err(unavailable());}
            Ok(Denomination{currency:self.requested.clone(),rate})
        }).transpose()
    }
}

pub async fn prepare(app:&App,owner:i64)->Result<Prepared,WebError>{
    let (requested,identity,config)=app.db(move|c|{
        let requested:String=c.query_row("SELECT display_currency FROM users WHERE id=?1",[owner],|r|r.get(0))?;
        Ok((figures::totals::normalized_currency(&requested),crate::engine::provider::fingerprint(c).map_err(|_|unavailable())?,crate::engine::provider::config(c).map_err(|_|unavailable())?))
    }).await?;
    let usd=||Prepared{identity:identity.clone(),requested:requested.clone(),rate:Some("1.0".into()),until:i64::MAX};
    if requested=="USD" {return Ok(usd());}
    let(Some(identity_key),Some(config))=(identity.clone(),config)else{return Ok(Prepared{identity,requested,rate:None,until:i64::MIN});};
    let store=app.fx_cache.store.clone();
    let held=match store.get(&identity_key,"USD",&requested,app.now().timestamp()) {
        Some(held)=>held,
        None=>{
            fill(app,&identity_key,Some(config),vec![("USD".into(),requested.clone())]).await?;
            store.get(&identity_key,"USD",&requested,app.now().timestamp()).ok_or_else(unavailable)?
        }
    };
    let rate=match held.value {
        Ok(HeldRate::Float(f))=>Some(f),
        Ok(HeldRate::Decimal(_))|Err(_)=>None,
    };
    // `Denomination.for`: the rate is `result.data.to_d`, a Float read as a decimal.
    let rate=rate.and_then(|f|figures::budget::within(||Dec::from_f64(f).map(|d|d.to_s_f())).ok());
    Ok(Prepared{identity,requested,rate,until:held.until})
}

/// The display currency's rate for a page, read from the held conversions only: a request never asks the network. A
/// conversion not held for this provider, or held past its time, is wanted from the figures queue (`Service::want_fx`),
/// which fills it outside any request; until then the page states "Currency conversion unavailable".
pub fn held(app:&App,c:&Connection,owner:i64)->Result<Prepared,WebError>{
    let requested:String=c.query_row("SELECT display_currency FROM users WHERE id=?1",[owner],|r|r.get(0))?;
    let requested=figures::totals::normalized_currency(&requested);
    let identity=crate::engine::provider::fingerprint(c).map_err(|_|unavailable())?;
    if requested=="USD" {return Ok(Prepared{identity,requested,rate:Some("1.0".into()),until:i64::MAX});}
    let configured=crate::engine::provider::config(c).map_err(|_|unavailable())?.is_some();
    let (Some(identity_key),true)=(identity.clone(),configured) else {return Ok(Prepared{identity,requested,rate:None,until:i64::MIN})};
    let Some(held)=app.fx_cache.store.get(&identity_key,"USD",&requested,app.now().timestamp()) else {
        app.figure_service.want_fx("USD",&requested);
        return Ok(Prepared{identity,requested,rate:None,until:i64::MIN});
    };
    let rate=match held.value {
        Ok(HeldRate::Float(f))=>Some(f),
        Ok(HeldRate::Decimal(_))|Err(_)=>None,
    };
    // `Denomination.for`: the rate is `result.data.to_d`, a Float read as a decimal.
    let rate=rate.and_then(|f|figures::budget::within(||Dec::from_f64(f).map(|d|d.to_s_f())).ok());
    Ok(Prepared{identity,requested,rate,until:held.until})
}
