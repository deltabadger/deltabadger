//! Current fiat rates. Fetch outside the database transaction; recheck configuration before saving.
use crate::{figures::{self,at::At,db::Ticker,dec::Dec,market::{Failure,Fetch,MarketData,Member,Quoted,Venue},num::{Num,NumError},totals::Denomination},jobs::data_api::{ApiError,DataApi},web::{App,WebError}};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

pub const SUCCESS_TTL:i64=12*60*60;
pub const FAILURE_TTL:i64=5*60;
#[derive(Clone)]
struct Entry {until:i64,rate:Option<String>}
#[derive(Default)]
pub struct Cache(tokio::sync::Mutex<BTreeMap<(String,String),Entry>>);
#[derive(Clone)]
pub struct Prepared {identity:Option<String>,requested:String,rate:Option<String>,until:i64}
fn unavailable()->WebError {WebError::Config("Currency conversion unavailable".into())}
struct Rates(Value);
impl MarketData for Rates {
    fn prices(&self,_:&Venue,_:&[String])->Fetch<Vec<(String,Member<Dec>)>> {Err(Failure::Failed("Market data unavailable".into()))}
    fn candles(&self,_:&Venue,_:&Ticker,_:At,_:i64,_:bool)->Fetch<Vec<(At,Dec)>> {Err(Failure::Failed("Market data unavailable".into()))}
    fn coin_price(&self,_:&str,_:&str)->Fetch<Quoted> {Err(Failure::Failed("Market data unavailable".into()))}
    fn exchange_rates(&self)->Fetch<Vec<(String,Member<Num>)>> {
        let Some(rates)=self.0.as_object() else{return Err(Failure::Failed("Currency conversion unavailable".into()))};
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
    let(Some(identity_key),Some(config))=(identity.as_ref(),config)else{return Ok(Prepared{identity,requested,rate:None,until:i64::MIN});};
    // Five supported currencies; cap identities as well. Holding this async lock gives concurrent
    // requests one fill, without holding App::db or starting detached work that could outlive it.
    let mut cache=app.fx_cache.0.lock().await;
    let key=(identity_key.clone(),requested.clone());let now=app.now().timestamp();
    if let Some(entry)=cache.get(&key).filter(|e|e.until>now){return Ok(Prepared{identity,requested,rate:entry.rate.clone(),until:entry.until});}
    let result=if matches!(app.figure_source,crate::web::figure::loading::Source::Script(_)) {
        let credentials=crate::crypto::Credentials{key:String::new(),secret:String::new(),passphrase:None};
        let wire=||crate::web::figure::loading::Wire::new(&credentials,app.figure_source.clone());
        DataApi::new(config,wire(),wire()).exchange_rates().await
    } else {DataApi::live(config).exchange_rates().await};
    let rates=match result {
        Ok(rates)=>rates,
        Err(ApiError::Failed{..})=>Value::Null,
        Err(ApiError::Transient(_))=>Value::Null,
    };
    let currency=requested.clone();
    let rate=app.db(move|c|figures::budget::within(||{
        match figures::totals::denomination(c,&Rates(rates),&currency) {
            Ok(d)=>Ok(Some(d.rate.to_s_f())),
            Err(figures::FiguresError::NotComputed(_)|figures::FiguresError::Raised(_))=>Ok(None),
            Err(_)=>Err(unavailable()),
        }
    })).await?;
    let ttl=if rate.is_some(){SUCCESS_TTL}else{FAILURE_TTL};
    if cache.len()>=32 {cache.retain(|_,e|e.until>now);}
    if cache.len()>=32 {cache.clear();}
    let until=app.now().timestamp().saturating_add(ttl);
    cache.insert(key,Entry{until,rate:rate.clone()});
    Ok(Prepared{identity,requested,rate,until})
}
