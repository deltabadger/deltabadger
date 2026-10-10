//! The four read tools. Network work runs outside App::db; accounting uses the merged figures library.
use super::{protocol::tool_text,read_limits,tools::{self,Called}};
use crate::{crypto::Credentials,figures::{self,at::At,budget,db::Subject,num::{Num,float_to_s},walk,live,totals,page_market::{Cache,Reader}},sync,venue::alpaca::{AlpacaVenue,Urls},web::{App,WebError,figure::{self,loading::Wire}}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;
pub const NAMES:[&str;4]=["get_exchange_balances","list_open_orders","get_bot_details","get_portfolio_summary"];
const ONLY:&str="this build reads Alpaca only";
fn error()->WebError{WebError::Config("MCP read unavailable".into())}
fn done(text:impl AsRef<str>)->Called{Called::Done(tool_text(read_limits::text(text.as_ref()),false))}
#[derive(Clone)]
pub struct Material { credentials:Credentials, origin:crate::engine::model::CredentialVersion }
impl Material {
    pub fn load(c:&Connection,cipher:&crate::crypto::Cipher,id:i64)->Result<Self,WebError>{
        let (credentials,origin)=sync::credentials_with_version(c,cipher,id).map_err(|_|error())?;
        Ok(Self{credentials,origin})
    }
}
impl std::ops::Deref for Material { type Target=Credentials;fn deref(&self)->&Credentials{&self.credentials} }
pub enum Fetch {
    Bound{inner:Box<Fetch>,versions:Vec<crate::engine::model::CredentialVersion>},
    Balances{user:i64,exchange:i64,name:String,credentials:Material},
    Orders{user:i64,local:Vec<String>,ids:HashSet<String>,venues:Vec<(i64,String,Option<Material>)>},
    Summary{user:i64,credentials:Option<Material>,now:At},
}
impl Fetch {
    fn producers(&self)->Vec<crate::engine::model::CredentialVersion>{
        match self {
            Self::Bound{versions,..}=>versions.clone(),
            Self::Balances{credentials,..}=>vec![credentials.origin.clone()],
            Self::Orders{venues,..}=>venues.iter().filter_map(|(_,_,c)|c.as_ref().map(|c|c.origin.clone())).collect(),
            Self::Summary{credentials,..}=>credentials.iter().map(|c|c.origin.clone()).collect(),
        }
    }
}
type RawOrders = Vec<Box<serde_json::value::RawValue>>;
pub enum Fetched { Bound(Box<Fetched>,Vec<crate::engine::model::CredentialVersion>), Text(String,bool), Balances(i64,i64,String,crate::ruby::BigDec,Vec<sync::balances::Position>), Orders(i64,Vec<String>,HashSet<String>,Vec<(i64,String,Result<RawOrders,String>)>), Summary(i64,Cache,At,bool) }
fn credentials(c:&Connection,app:&App,user:i64,exchange:i64)->Result<Option<Material>,WebError>{
    let id=c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 AND status=1 LIMIT 1",[user,exchange],|r|r.get(0)).optional()?;
    id.map(|id|sync::credentials_with_version(c,&app.cipher,id).map(|(credentials,origin)|Material{credentials,origin}).map_err(|_|error())).transpose()
}
fn exchange(c:&Connection,name:&str)->Result<Option<(i64,String,String)>,WebError>{Ok(c.query_row("SELECT id,name,type FROM exchanges WHERE lower(name)=?1 AND type!='Exchanges::Bitmart' LIMIT 1",[name.to_lowercase()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)}
fn unknown(c:&Connection,name:&str)->Result<Called,WebError>{Ok(done(format!("Exchange '{name}' not found. Available exchanges: {}",super::tradeable(c)?.join(", "))))}
pub fn plan(c:&Connection,app:&App,user:i64,name:&str,args:&Value)->Result<Called,WebError>{
    let tx=c.unchecked_transaction()?;
    let planned=plan_inner(&tx,app,user,name,args)?;
    let out=match planned {
        Called::Fetch(inner)=>{
            let versions=inner.producers();
            Called::Fetch(Fetch::Bound{inner:Box::new(inner),versions})
        }
        done=>done,
    };
    tx.commit()?;Ok(out)
}
fn plan_inner(c:&Connection,app:&App,user:i64,name:&str,args:&Value)->Result<Called,WebError>{
    if !read_limits::check(c,user)?{return Ok(done(read_limits::REFUSAL))}
    let now=At::from_utc(app.now()).ok_or_else(error)?;
    match name {
        "get_bot_details"=>Ok(done(budget::within(||details(c,user,args["bot_id"].as_f64().unwrap_or(0.0) as i64,now))?)),
        "get_exchange_balances"=>{
            let name=args["exchange_name"].as_str().unwrap_or("");
            let Some((exchange,name,kind))=exchange(c,name)? else{return unknown(c,name)};
            let Some(credentials)=credentials(c,app,user,exchange)? else{return Ok(done(format!("No valid API key found for {name}. Please add an API key in Settings.")))};
            if kind!="Exchanges::Alpaca" {return Ok(done(ONLY))}
            if credentials.passphrase.as_deref()==Some("live"){return Ok(done("Balances unavailable: this build reads Alpaca paper only"))}
            Ok(Called::Fetch(Fetch::Balances{user,exchange,name,credentials}))
        },
        "list_open_orders"=>{
            let filter=if let Some(name)=args["exchange_name"].as_str().filter(|s|!s.trim().is_empty()) {
                match exchange(c,name)?{Some(ex)=>Some(ex),None=>return Ok(done(format!("Exchange '{name}' not found. Available: {}",super::tradeable(c)?.join(", "))))}
            }else{None};
            let (local,ids)=local_orders(c,user,filter.as_ref().map(|x|x.0))?;
            if local==[read_limits::REFUSAL]{return Ok(done(read_limits::REFUSAL))}
            let exchanges=if let Some(ex)=filter {vec![ex]}else{
                c.prepare("SELECT DISTINCT e.id,e.name,e.type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0 AND k.status=1 ORDER BY k.id")?.query_map([user],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?
            };
            let mut venues=vec![];
            for (id,name,kind) in exchanges {if kind=="Exchanges::Alpaca" {venues.push((id,name,credentials(c,app,user,id)?));}}
            Ok(Called::Fetch(Fetch::Orders{user,local,ids,venues}))
        },
        "get_portfolio_summary"=>{
            let n:i64=c.query_row("SELECT count(*) FROM bots WHERE user_id=?1 AND status!=3",[user],|r|r.get(0))?;
            if n==0 {return Ok(done("No bots found. Create a bot to start tracking your portfolio."))}
            let refused=budget::within(||->Result<bool,WebError>{
                for id in ids(c,user)?{
                    let Some(b)=bot(c,user,id)? else{continue};
                    let (subject,metrics)=match metrics(c,&b,now){Ok(v)=>v,Err(WebError::Config(why)) if why=="executed fill value unavailable"=>continue,Err(e)=>return Err(e)};
                    let needs_price=if pair_kind(&b.kind){!metrics.chart.labels.is_empty()&&!subject.tickers.is_empty()}else{live::needs_venue_price(c,&subject,&metrics,now).map_err(fail_fig)?};
                    if subject.bot.exchange_type.as_deref()!=Some("Exchanges::Alpaca")&&needs_price{return Ok(true)}
                }Ok(false)
            })?;
            if refused{return Ok(done(ONLY))}
            let credentials=figure::loading::read_info(c,app,user)?.map(|i|Material{credentials:i.credentials,origin:i.origin});
            Ok(Called::Fetch(Fetch::Summary{user,credentials,now}))
        },_=>Err(error())
    }
}
async fn balance_read(venue:&AlpacaVenue<Wire>)->Result<(crate::ruby::BigDec,Vec<sync::balances::Position>),(String,bool)>{
    let cash=sync::parsed(venue.read(false,"/v2/account",vec![],sync::balances::MAX_ACCOUNT_BYTES).await.map_err(sync::venue_failure)?,sync::balances::account).await?;
    let held=sync::parsed(venue.read(false,"/v2/positions",vec![],sync::balances::MAX_LIST_BYTES).await.map_err(sync::venue_failure)?,sync::balances::positions).await?;
    match (cash,held){(Ok(c),Ok(h))=>Ok((c,h)),(Err(e),_)|(_,Err(e))=>Err((e,false))}
}
pub async fn fetch(app:&App,fetch:Fetch)->Fetched{
    let versions=fetch.producers();
    let value=fetch_inner(app,fetch).await;
    if matches!(value,Fetched::Bound(..)){value}else{Fetched::Bound(Box::new(value),versions)}
}
async fn fetch_inner(app:&App,fetch:Fetch)->Fetched{
    match fetch{
        Fetch::Bound{inner,versions}=>Fetched::Bound(Box::new(Box::pin(self::fetch_inner(app,*inner)).await),versions),
        Fetch::Balances{user,exchange,name,credentials}=>{
            let venue=AlpacaVenue::new(Wire::new(&credentials,app.figure_source.clone()).with_origin(credentials.origin.clone()),Urls::for_passphrase(credentials.passphrase.as_deref()));
            match balance_read(&venue).await {Ok((cash,held))=>Fetched::Balances(user,exchange,name,cash,held),Err((_,true))=>Fetched::Text("An unexpected error occurred.".into(),true),Err((why,false))=>Fetched::Text(format!("Failed to fetch balances from {name}: {}",sync::scrub(&why,&credentials)),false)}
        },
        Fetch::Orders{user,local,ids,venues}=>{
            let mut out=vec![];
            for (id,name,credentials) in venues {
                let Some(credentials)=credentials else{out.push((id,name,Err("no usable trading key".into())));continue};
                if credentials.passphrase.as_deref()==Some("live"){out.push((id,name,Err("this build reads Alpaca paper only".into())));continue}
                let venue=AlpacaVenue::new(Wire::new(&credentials,app.figure_source.clone()).with_origin(credentials.origin.clone()),Urls::for_passphrase(None));
                let result=match venue.read(false,"/v2/orders",vec![("status","open".into()),("limit","50".into())],sync::balances::MAX_LIST_BYTES).await {
                    Err(e)=>Err(sync::venue_failure(e)),
                    Ok(body)=>sync::parsed(body,|text|serde_json::from_str::<RawOrders>(text).map_err(|_|sync::Unread::Raised("Unreadable orders".into()))).await
                };
                match result {Ok(v)=>out.push((id,name,Ok(v))),Err((_,true))=>return Fetched::Text("An unexpected error occurred.".into(),true),Err((why,false))=>out.push((id,name,Err(sync::scrub(&why,&credentials))))}
            }
            Fetched::Orders(user,local,ids,out)
        },
        Fetch::Summary{user,credentials,now}=>{
            let mut cache=Cache::default();
            let ready=if let Some(credentials)=credentials{
                let wire=Wire::new(&credentials,app.figure_source.clone()).with_origin(credentials.origin.clone());
                figure::loading::fill_with(app,user,&mut cache,&wire,now,read_limits::check,move|c,r|global(c,user,r,now).is_ok()).await
            }else{true}; // An empty market cache can still compute local cash-only USD totals.
            Fetched::Summary(user,cache,now,ready)
        }
    }
}
pub fn finish(c:&Connection,fetch:Fetched)->Result<Value,WebError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=finish_in(&tx,fetch)?;tx.commit()?;Ok(out)}else{finish_in(c,fetch)}
}
/// Every envelope and derived reply consumes the same finishing SQLite snapshot.
fn finish_in(c:&Connection,fetch:Fetched)->Result<Value,WebError>{
    if let Fetched::Bound(inner,versions)=fetch {
        for version in versions {
            if !crate::engine::model::credential_is_current(c,&version)? {
                crate::engine::log(crate::engine::model::CREDENTIALS_CHANGED);
                return Ok(tool_text("Figures unavailable: credentials changed; retry with current credentials",false));
            }
        }
        return finish_in(c,*inner)
    }

    let user=match &fetch{Fetched::Balances(u,..)|Fetched::Orders(u,..)|Fetched::Summary(u,..)=>Some(*u),Fetched::Text(..)|Fetched::Bound(..)=>None};
    if let Some(user)=user{if !read_limits::check(c,user)?{return Ok(tool_text(read_limits::REFUSAL,false))}}
    let text=match fetch{
        Fetched::Bound(..)=>return Err(error()),
        Fetched::Text(text,is_error)=>return Ok(tool_text(read_limits::text(&text),is_error)),
        Fetched::Balances(user,exchange,name,cash,held)=>{
            let catalog=sync::balances::catalog_for(c,user,exchange).map_err(|_|error())?;
            let mut lines=vec![];
            for (id,qty) in match sync::balances::complete_balances(&catalog,cash,held){Ok(rows)=>rows,Err(why)=>return Ok(tool_text(&format!("Failed to fetch balances from {name}: {why}"),false))}{
                let n=qty.to_f(); if n==0.0{continue}
                let symbol:Option<String>=c.query_row("SELECT symbol FROM assets WHERE id=?1",[id],|r|r.get(0)).optional()?.flatten();
                lines.push(format!("- {}: {}",symbol.unwrap_or_else(||format!("Unknown({id})")),float_to_s(n)));
            }
            if lines.is_empty(){format!("All balances on {name} are zero.")}else{format!("{name} Balances:\n{}",lines.join("\n"))}
        },
        Fetched::Orders(_user,mut local,ids,venues)=>{
            let mut unavailable=vec![];
            for (exchange,name,result) in venues{
                match result{
                    Err(why)=>unavailable.push(format!("! {name}: could not be checked ({why}) — open orders there are not listed")),
                    Ok(rows)=>{
                        if !read_limits::count(rows.len(),read_limits::VENUE_ORDERS){return Ok(tool_text(read_limits::REFUSAL,false))}
                        for raw in rows{
                        let (id,pair,class)=crate::venue::alpaca::order_identity(raw.get());
                        let id=id.as_deref().unwrap_or("");if ids.contains(id){continue}
                        let Some(ticker)=crate::engine::model::alpaca_order_ticker(c,exchange,pair.as_deref(),class.as_deref()).map_err(|_|error())? else {
                            eprintln!("Alpaca order skipped: unsupported class or unmapped/ambiguous order identity");
                            continue;
                        };
                        let raw=crate::venue::http::decode_json(raw.get()).map_err(|_|error())?;
                        let parsed=crate::venue::alpaca::parse_read_order(id,&raw).map_err(|_|error())?;
                        let pair=format!("{}/{}",ticker.base_code,ticker.quote_code);
                        let amount=parsed.amount.map(|a|a.to_s_f()).unwrap_or_else(||"N/A".into());
                        let price=parsed.price.map(|p|format!("@ {}",p.to_s_f())).unwrap_or_default(); // Rails omits the price only when nil.
                        let side=raw["side"].as_str().map(str::to_uppercase).unwrap_or_else(||"?".into());
                        local.push(format!("- {side} {amount} {pair} {price} ({}) | {name} | ext: {id}",if parsed.limit{"Limit order"}else{"Market order"}));
                    }}
                }
            }
            if local.is_empty()&&unavailable.is_empty(){"No open orders found.".into()}else{
                let header=if local.is_empty(){"No open orders found on the exchanges that answered.".into()}else{format!("Open orders ({}):",local.len())};
                [vec![header],local,unavailable].concat().join("\n")
            }
        },
        Fetched::Summary(user,cache,now,ready)=>budget::within(||summary(c,user,&cache,now,ready))?,
    };Ok(tool_text(read_limits::text(&text),false))
}
fn local_orders(c:&Connection,user:i64,exchange:Option<i64>)->Result<(Vec<String>,HashSet<String>),WebError>{
    let n:usize=c.query_row("SELECT count(*) FROM (SELECT t.id FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND t.status=0 AND t.external_status=1 AND (?2 IS NULL OR t.exchange_id=?2) LIMIT 101)",rusqlite::params![user,exchange],|r|r.get(0))?;
    if !read_limits::count(n,read_limits::LOCAL_ORDERS){return Ok((vec![read_limits::REFUSAL.into()],HashSet::new()))}
    let zone:String=c.query_row("SELECT time_zone FROM users WHERE id=?1",[user],|r|r.get(0))?;
    let mut q=c.prepare("SELECT t.id,t.created_at,t.side,t.amount,t.base,t.quote,t.price,t.order_type,e.name,t.external_id FROM transactions t JOIN bots b ON b.id=t.bot_id JOIN exchanges e ON e.id=t.exchange_id WHERE b.user_id=?1 AND t.status=0 AND t.external_status=1 AND (?2 IS NULL OR t.exchange_id=?2) ORDER BY t.created_at DESC LIMIT 100")?;
    let rows=q.query_map(rusqlite::params![user,exchange],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,tools::number(r,3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,tools::price(r,6)?,r.get::<_,Option<i64>>(7)?,r.get::<_,String>(8)?,r.get::<_,Option<String>>(9)?)))?;
    let mut lines=vec![];let mut ids=HashSet::new();
    for row in rows{
        let (id,time,side,amount,base,quote,price,kind,name,ext)=row?;
        let date=crate::codec::parse_time(&time).map_err(|_|error())?;
        let date=crate::web::timezone::local(date,&zone).format("%Y-%m-%d %H:%M");
        let ext=ext.unwrap_or_default(); // nil interpolates to the empty string in Rails.
        if !ext.trim().is_empty(){ids.insert(ext.clone());}
        // Optional DB columns interpolate as empty; an absent amount alone prints N/A.
        lines.push(format!("- [{date}] #{id} {} {} {}/{} {} ({}) | {name} | ext: {ext}",match side{Some(0)=>"BUY",Some(1)=>"SELL",_=>""},amount.unwrap_or_else(||"N/A".into()),base.unwrap_or_default(),quote.unwrap_or_default(),price.map(|v|format!("@ {v}")).unwrap_or_default(),match kind{Some(0)=>"Market order",Some(1)=>"Limit order",_=>"Unknown"}));
    }Ok((lines,ids))
}
fn num(n:&Num)->Result<String,WebError>{
    Ok(match n{Num::Int(i)=>i.to_string(),Num::Float(f)=>float_to_s(sync::balances::float_round(*f,2)),Num::Dec(d)=>d.round(2).map_err(|_|error())?.to_s_f()})
}
fn pct(n:&Num)->Result<String,WebError>{Ok(format!("{}{}%",if n.is_negative(){""}else{"+"},num(&n.mul(&Num::Int(100)).map_err(|_|error())?)?))}
fn fail_fig(e:figures::FiguresError)->WebError{match e{figures::FiguresError::Data(ref why)|figures::FiguresError::NotComputed(ref why) if why=="executed fill value unavailable"||why=="quote currency unavailable"=>WebError::Config(why.clone()),_=>error()}}
struct Bot {id:i64,label:String,kind:String,status:i64,settings:Value,transient:Value,exchange:String,quote:String,base:Option<String>,pair:Option<String>,started:Option<String>}
fn bot(c:&Connection,user:i64,id:i64)->Result<Option<Bot>,WebError>{
    read_limits::charge_bot()?;
    let row=c.query_row("SELECT b.label,b.type,b.status,b.settings,e.name,b.started_at,b.transient_data FROM bots b LEFT JOIN exchanges e ON e.id=b.exchange_id WHERE b.id=?1 AND b.user_id=?2 AND b.status!=3",[id,user],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?))).optional()?;
    let Some((label,kind,status,settings,exchange,started,transient))=row else{return Ok(None)};
    let settings:Value=serde_json::from_str(&settings).map_err(|_|error())?;
    let transient:Value=serde_json::from_str(&transient).map_err(|_|error())?;
    let quote=tools::asset(c,&settings["quote_asset_id"])?.unwrap_or_default(); // nil quote interpolates as empty.
    let members=members(c,&kind,&settings)?;
    let base=if pair_kind(&kind){tools::asset(c,&settings["base_asset_id"])?}else if members.len()==1{Some(members[0].1.clone())}else{None};
    let pair=if kind=="Bots::DcaMultiAsset"{Some(format!("{}/{}",members.iter().map(|(_,s,_)|s.as_str()).collect::<Vec<_>>().join("+"),quote))}else if pair_kind(&kind){base.as_ref().filter(|_|!quote.is_empty()).map(|base|format!("{base}/{quote}"))}else{None};
    let label=match label.filter(|s|!s.trim().is_empty()){
        Some(label)=>label,
        None=>generated_label(c,user,id,&kind,&settings,"en")?,
    };
    Ok(Some(Bot{id,label,kind,status,settings,transient,exchange:exchange.unwrap_or_else(||"N/A".into()),quote,base,pair,started}))
}
/// Automation::Labelable#generate_label for a bot stored without a name, any type. Only a read's
/// view of it: nothing is saved. An index bot is named by `Bot::find`, which names what it loads.
pub(crate) fn generated_label(c:&Connection,user:i64,id:i64,kind:&str,s:&Value,locale:&str)->Result<String,WebError>{
    let label=if kind=="Bots::DcaIndex"{
        crate::web::bot::Bot::find(c,user,id,crate::web::bot::For::Feed,locale)?.ok_or_else(error)?.label
    }else{
        let ids=if pair_kind(kind){s["base_asset_id"].as_i64().into_iter().collect::<Vec<_>>()}else if let Some(a)=s["allocations"].as_object().filter(|a|!a.is_empty()){a.keys().filter_map(|k|crate::web::bot::id_from_path(k)).collect()}else{s["base_asset_ids"].as_array().into_iter().flatten().map(|v|v.as_i64().unwrap_or(0)).collect()};
        let assets=figures::db::asset_names(c,&ids).map_err(fail_fig)?;
        if ids.len()==1{assets.first().and_then(|(_,_,name)|name.clone()).unwrap_or_default()}else{ // allow-swallow: absent asset/name is an Option; Rails generates bot.new below.
            let symbols=ids.iter().filter_map(|id|assets.iter().find(|(asset,_,_)|asset==id).and_then(|(_,symbol,_)|symbol.as_deref())).collect::<Vec<_>>();
            let label=symbols.iter().take(3).copied().collect::<Vec<_>>().join(", ");
            if symbols.len()>3{format!("{label} + {}",symbols.len()-3)}else{label}
        }
    };
    Ok(if label.trim().is_empty(){crate::web::i18n::text(locale,"bot.new",&[])}else{label})
}
fn members(c:&Connection,kind:&str,s:&Value)->Result<Vec<(i64,String,f64)>,WebError>{
    if kind!="Bots::DcaMultiAsset"{return Ok(vec![])}
    let mut out=vec![];
    let ids=if let Some(a)=s["allocations"].as_object().filter(|a|!a.is_empty()){a.keys().filter_map(|k|crate::web::bot::id_from_path(k)).collect::<Vec<_>>()}else{s["base_asset_ids"].as_array().into_iter().flatten().map(|v|v.as_i64().unwrap_or(0)).collect()};
    let assets=figures::db::asset_names(c,&ids).map_err(fail_fig)?;
    for id in ids{
        if let Some((_,symbol,_))=assets.iter().find(|(asset,_,_)|*asset==id){
            let symbol=symbol.clone().unwrap_or_default(); // nil symbol joins as empty in a pair.
            let weight=s["allocations"][id.to_string()].as_f64().unwrap_or(0.0); // nil.to_f in allocation_for.
            out.push((id,symbol,weight));
        }
    }Ok(out)
}
fn pair_kind(kind:&str)->bool{matches!(kind,"Bots::DcaSingleAsset"|"Bots::Signal")}
fn metrics(c:&Connection,b:&Bot,now:At)->Result<(Subject,walk::Metrics),WebError>{
    if pair_kind(&b.kind){let p=figures::pair::metrics(c,b.id,now).map_err(fail_fig)?;return Ok((p.subject,p.metrics));}
    let s=Subject::load(c,b.id).map_err(fail_fig)?;
    let m=walk::metrics(c,&s,now).map_err(fail_fig)?;
    Ok((s,m))
}
fn effective_started(b:&Bot,zone:&str)->Result<Option<chrono::DateTime<chrono::Utc>>,WebError>{
    let Some(raw)=&b.started else{return Ok(None)};
    let mut start=crate::codec::parse_time(raw).map_err(|_|error())?;
    // Signal and index have no trigger decorators.
    if matches!(b.kind.as_str(),"Bots::Signal"|"Bots::DcaIndex") {return Ok(Some(start))}
    let selling=b.settings["direction"]=="selling";
    let prefix=if selling{"sell_"}else{""};
    for trigger in ["price","price_drop","moving_average","indicator"]{
        if b.settings[format!("{prefix}{trigger}_limited")].as_bool()!=Some(true){continue}
        let Some(raw)=b.transient[format!("{prefix}{trigger}_limit_condition_met_at")].as_str().filter(|s|!s.trim().is_empty()) else{return Ok(None)};
        // ActionMCP::Current sets Time.zone to the calling user's zone.
        let met=match chrono::DateTime::parse_from_rfc3339(raw){
            Ok(t)=>t.to_utc(),
            Err(_)=>{
                let local=chrono::NaiveDateTime::parse_from_str(&raw.replacen('T'," ",1),"%Y-%m-%d %H:%M:%S%.f").map_err(|_|error())?;
                use chrono::TimeZone;
                // Rails prefers the DST occurrence at an ambiguous local time; invalid gaps refuse.
                crate::web::timezone::zone(zone).unwrap_or(chrono_tz::Tz::UTC).from_local_datetime(&local).earliest().ok_or_else(error)?.to_utc()
            },
        };
        start=start.max(met);
    }
    Ok(Some(start))
}
fn details(c:&Connection,user:i64,id:i64,now:At)->Result<String,WebError>{
    let Some(b)=bot(c,user,id)?else{return Ok("Bot not found.".into())};
    let status=tools::STATUSES.get(usize::try_from(b.status).map_err(|_|error())?).ok_or_else(error)?;
    let mut lines=vec![format!("Bot: {}",b.label),format!("Type: {}",tools::type_name(&b.kind)),format!("Status: {status}"),format!("Exchange: {}",b.exchange)];
    if let Some(pair)=&b.pair{lines.push(format!("Pair: {pair}"));}
    let members=members(c,&b.kind,&b.settings)?;
    if !members.is_empty(){
        let names=figures::db::asset_names(c,&members.iter().map(|(id,_,_)|*id).collect::<Vec<_>>()).map_err(fail_fig)?;
        let candidates=names.iter().map(|(id,symbol,name)|(figures::keys::Identity::Asset(*id),figures::keys::candidate(*id,symbol.as_deref(),name.as_deref()))).collect::<Vec<_>>();
        let keys=figures::keys::call(&candidates).map_err(|_|error())?;
        let formatted=members.iter().map(|(id,_,w)|{
            let key=keys.iter().find(|(asset,_)|*asset==figures::keys::Identity::Asset(*id)).map(|(_,key)|key).ok_or_else(error)?;
            Ok(format!("{key} {:.2}%",w*100.0))
        }).collect::<Result<Vec<_>,WebError>>()?;
        lines.push(format!("Allocations: {}",formatted.join(", ")));
    }
    let result=metrics(c,&b,now);
    let mut redeploy_line=None;
    if let Ok((s,m))=&result {if !pair_kind(&b.kind){
        let in_index=c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1 AND in_index=1")?.query_map([id],|r|r.get(0))?.collect::<Result<HashSet<i64>,_>>()?;
        let mut held=vec![];let mut exited=vec![];
        for (key,row) in &m.asset_breakdown {
            let asset=m.key_assets.iter().find(|(k,_)|k==key).and_then(|(_,id)|*id);
            if let Some(t)=live::ticker_for_key(s,m,key).filter(|_|asset.is_some()){
                let value=c.query_row("SELECT minimum_base_size FROM tickers WHERE id=?1",[t.id],|r|r.get::<_,rusqlite::types::Value>(0))?;
                let minimum=crate::figures::dec::Dec::from_sql((&value).into()).map_err(|_|error())?.unwrap_or_else(figures::dec::Dec::zero); // nil.to_d is zero.
                if row.amount.to_d().map_err(|_|error())?>=minimum{
                    held.push(key.clone());if !in_index.is_empty()&&asset.is_some_and(|id|!in_index.contains(&id)){exited.push(key.clone());}
                }
            }
        }
        if !held.is_empty(){lines.push(format!("Holdings (sellable with liquidate_exited_asset): {}",held.join(", ")));}
        if !exited.is_empty(){lines.push(format!("Exited holdings: {}",exited.join(", ")));}
        if figures::db::composition_minimum(c,s).map_err(fail_fig)?.is_none(){
            redeploy_line=Some("Redeploy unavailable: no composition minimum".into());
        }else if let Some(offer)=figure::redeploy_offer(c,s,m,&in_index).map_err(fail_fig)?.filter(|v|v.is_positive()){
            redeploy_line=Some(format!("Redeploy offer (answer with answer_redeploy_offer): {} {}",offer.to_s_f(),b.quote));
        }
    }
    }
    let (enabled,jurisdiction,zone):(Option<bool>,Option<String>,String)=c.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction,time_zone FROM users WHERE id=?1",[user],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let jurisdiction=jurisdiction.as_deref().filter(|s|!s.trim().is_empty()).unwrap_or("US");
    if enabled==Some(true)&&["US","GB","IE"].contains(&jurisdiction){
        let membership=if pair_kind(&b.kind){
            "SELECT t.base_asset_id FROM tickers t JOIN bots b ON b.exchange_id=t.exchange_id WHERE b.id=?2 AND t.base_asset_id=json_extract(b.settings,'$.base_asset_id') AND t.quote_asset_id=json_extract(b.settings,'$.quote_asset_id')"
        }else{"SELECT asset_id FROM bot_index_assets WHERE bot_id=?2"};
        let mut q=c.prepare(&format!("SELECT a.symbol,l.buy_locked_until FROM wash_sale_locks l JOIN assets a ON a.id=l.asset_id WHERE l.user_id=?1 AND l.asset_id IN ({membership}) ORDER BY l.id"))?;
        let mut locks=vec![];
        for row in q.query_map([user,id],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?)))?{
            let (symbol,until)=row?;
            if let Some(until)=until{
                let until=crate::codec::parse_time(&until).map_err(|_|error())?;
                if until>now.utc(){let days=(crate::web::timezone::local(until,&zone).date_naive()-crate::web::timezone::local(now.utc(),&zone).date_naive()).num_days();locks.push(format!("{} {days}d",symbol.unwrap_or_default()));} // nil symbol interpolates empty, as Rails.
            }
        }
        if !locks.is_empty(){lines.push(format!("Locked out of buying (wash sale): {}",locks.join(", ")));}
    }
    if let Some(line)=redeploy_line{lines.push(line);}
    lines.push(format!("Interval: {}",b.settings["interval"].as_str().unwrap_or("N/A")));
    lines.push(format!("Amount per order: {} {}",tools::str_value(&b.settings["quote_amount"]),b.quote));
    let count:i64=c.query_row("SELECT count(*) FROM transactions WHERE bot_id=?1 AND status IN (0,2)",[id],|r|r.get(0))?;
    lines.push(format!("Orders executed: {count}"));
    if let Some(start)=effective_started(&b,&zone)?{
        lines.push(format!("Started: {}",crate::web::timezone::local(start,&zone).format("%Y-%m-%d %H:%M UTC")));
    }
    lines.push(String::new());
    match result{
        Err(WebError::Config(why)) if why=="executed fill value unavailable"=>lines.push("Metrics unavailable: executed fill value unavailable".into()),
        Err(_)=>lines.push("Metrics unavailable: metrics unavailable".into()),
        Ok((_,m))=>{
            lines.push("--- Performance ---".into());
            lines.push(format!("Total invested: {} {}",num(&m.total_quote_amount_invested)?,b.quote));
            lines.push(format!("Current value: {} {}",num(&m.total_amount_value_in_quote)?,b.quote));
            if let Some(p)=&m.pnl{lines.push(format!("P/L: {}",pct(p)?));}
            if pair_kind(&b.kind){
                let p=figures::pair::metrics(c,b.id,now).map_err(fail_fig)?;
                if let Some(avg)=p.average{lines.push(format!("Average buy price: {} {}",num(&avg)?,b.quote));}
                lines.push(format!("Total acquired: {} {}",num(&p.amount)?,b.base.as_deref().unwrap_or("units")));
            }else if let Some(base)=b.base{
                let identity=members.first().map(|m|m.0);
                let key=m.key_assets.iter().find(|(_,asset)|*asset==identity).map(|(key,_)|key);
                let row=m.asset_breakdown.iter().find(|(k,_)|Some(k)==key);
                let amount=row.map(|(_,r)|r.amount.clone()).unwrap_or(Num::Int(0));
                if let Some((_,r))=row.filter(|(_,r)|r.amount.is_positive()) {lines.push(format!("Average buy price: {} {}",num(&r.quote_invested.div(&r.amount).map_err(|_|error())?)?,b.quote));}
                lines.push(format!("Total acquired: {} {base}",num(&amount)?));
            }
        }
    }
    Ok(lines.join("\n"))
}
fn ids(c:&Connection,user:i64)->Result<Vec<i64>,WebError>{Ok(c.prepare("SELECT id FROM bots WHERE user_id=?1 AND status!=3 ORDER BY id")?.query_map([user],|r|r.get(0))?.collect::<Result<Vec<_>,_>>()?)}
fn global(c:&Connection,user:i64,reader:&Reader<'_>,now:At)->Result<Option<totals::GlobalPnl>,WebError>{
    let mut computed=vec![];
    for id in ids(c,user)?{
        read_limits::charge_bot()?;
        let kind:String=c.query_row("SELECT type FROM bots WHERE id=?1",[id],|r|r.get(0))?;
        if pair_kind(&kind){
            let p=figures::pair::marked(c,id,now,reader).map_err(fail_fig)?;
            if p.metrics.prices_stale{return Ok(None)}
            computed.push((p.subject,p.metrics));continue;
        }
        let s=Subject::load(c,id).map_err(fail_fig)?;
        let walked=walk::metrics(c,&s,now).map_err(fail_fig)?;
        let m=live::live(c,&s,&walked,reader,now).map_err(fail_fig)?;
        if m.prices_stale||!figure::missing(&s,&m,reader).map_err(fail_fig)?.is_empty(){return Ok(None)}
        computed.push((s,m));
    }
    let parts=computed.iter().map(|(s,m)|totals::Part{bot_id:s.bot.id,quote:s.quote.as_deref(),traded:!s.orders.is_empty(),figures:Ok(Some(m))}).collect::<Vec<_>>();
    totals::global_pnl(c,reader,&mut totals::Rates::default(),&parts).map_err(fail_fig)
}
fn summary(c:&Connection,user:i64,cache:&Cache,now:At,ready:bool)->Result<String,WebError>{
    let bots=ids(c,user)?.into_iter().map(|id|bot(c,user,id)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten().collect::<Vec<_>>();
    let count=|statuses:&[i64]|bots.iter().filter(|b|statuses.contains(&b.status)).count();
    let archived=count(&[7]);
    let mut lines=vec!["Portfolio Summary".into(),"================".into(),format!("Total bots: {} ({} active, {} stopped, {} not started{})",bots.len(),count(&[1,4,5,6]),count(&[2]),count(&[0]),if archived>0{format!(", {archived} archived")}else{String::new()}),String::new()];
    let reader=Reader::new(cache,now.utc().timestamp()).with_current(c).with_symbols(figure::loading::symbols(c,user).map_err(fail_fig)?);
    let reason=if bots.iter().any(|b|b.quote.trim().is_empty()){Some("quote currency unavailable")}
        else if bots.iter().any(|b|matches!(metrics(c,b,now),Err(WebError::Config(why)) if why=="executed fill value unavailable")){Some("executed fill value unavailable")}
        else{None};
    let pnl=if ready&&reason.is_none(){global(c,user,&reader,now)?}else{None};
    if let Some(p)=pnl{
        lines.push(format!("Global P/L: {}",pct(&p.percent)?));
        lines.push(format!("Profit (USD): {}${}",if p.profit_usd.is_negative(){""}else{"+"},num(&p.profit_usd)?));
    }else if let Some(why)=reason{
        lines.push(format!("Global P/L: Not available ({why})"));
        if why=="quote currency unavailable"{lines.push(format!("Profit (USD): Not available ({why})"));}
    }else{lines.push("Global P/L: Not available (needs market data)".into());}
    lines.extend([String::new(),"--- Per-Bot Summary ---".into()]);
    for b in bots{
        let name=format!("- {} ({}) | {} | ",b.label,b.pair.as_deref().unwrap_or("N/A"),tools::STATUSES.get(usize::try_from(b.status).map_err(|_|error())?).ok_or_else(error)?);
        lines.push(match metrics(c,&b,now){Ok((_,m))=>if let Some(p)=&m.pnl{format!("{name}P/L: {} | Invested: {} {}",pct(p)?,num(&m.total_quote_amount_invested)?,b.quote)}else{format!("{name}No metrics yet")},Err(WebError::Config(why)) if why=="executed fill value unavailable"=>format!("{name}Metrics unavailable: executed fill value unavailable"),Err(_)=>format!("{name}Metrics unavailable")});
    }
    Ok(lines.join("\n"))
}
