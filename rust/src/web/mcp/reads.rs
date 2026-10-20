//! The four read tools, and the BotApi services behind them (bot_api::Read: the data REST renders and the text MCP says,
//! from one snapshot). Network work runs outside App::db; accounting uses the merged figures library.
use super::{protocol::tool_text,read_limits,tools::{self,Called}};
use crate::{crypto::Credentials,figures::{self,at::At,budget,db::Subject,num::{Num,float_to_s},walk,live,totals,page_market::{Cache,Reader}},sync,venue::alpaca::{AlpacaVenue,Hosts,Urls},web::{App,WebError,bot_api::{Domain,Read,Res},figure::{self,loading::{Source,Wire}}}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::{json,Value};
use std::collections::HashSet;
pub const NAMES:[&str;4]=["get_exchange_balances","list_open_orders","get_bot_details","get_portfolio_summary"];
const ONLY:&str="this build reads Alpaca only";
fn error()->WebError{WebError::Config("MCP read unavailable".into())}
/// A read's success: the service's data and the tool's text.
fn ok(data:Value,text:impl Into<String>)->Read{Read{res:Res::success(data),text:text.into(),is_error:false}}
/// A BotApi failure, which the Rails tool reports with report_error (isError).
fn failed(status:Domain,code:&'static str,text:impl Into<String>)->Read{let text=text.into();Read{res:Res::failure(status,code,text.clone()),text,is_error:true}}
/// What this build does not read (a venue other than Alpaca) or bounds (a read past read_limits): the 501 `not_ported`
/// envelope on REST; on MCP the text it has always said, an error or not.
fn unported(text:&str,is_error:bool)->Read{Read{res:Res::failure(Domain::NotPorted,"not_ported",text.into()),text:text.into(),is_error}}
/// What Rails raises past the service (Client::TransientNetworkError): the MCP tool's rescue, REST's 500.
fn raised()->Read{Read{res:Res::failure(Domain::Raised,"internal_error","An unexpected error occurred.".into()),text:"An unexpected error occurred.".into(),is_error:true}}
/// The MCP tool's answer for a read.
pub fn tool_result(read:&Read)->Value{tool_text(read_limits::text(&read.text),read.is_error)}
fn done(read:Read)->Called{Called::Done(tool_result(&read))}
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
    Orders{user:i64,local:Vec<String>,rows:Vec<Value>,ids:HashSet<String>,venues:Vec<(i64,String,Option<Material>)>},
    /// `producer`: what the account's figures are made under (an Alpaca key or none, `loading::read_info`).
    Summary{user:i64,credentials:Option<Material>,producer:crate::figures::page_market::Producer,now:At},
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
pub enum Fetched { Bound(Box<Fetched>,Vec<crate::engine::model::CredentialVersion>), Done(Read), Balances(i64,i64,String,crate::ruby::BigDec,Vec<sync::balances::Position>), Orders(i64,Vec<String>,Vec<Value>,HashSet<String>,Vec<(i64,String,Result<RawOrders,String>)>), Summary(i64,Cache,At,bool) }
/// What a read asks for, as each surface names it: MCP by the tool's arguments, REST by its route's parameters.
pub enum Ask { Details(i64), Balances{name:String,id:Option<String>}, Orders(Option<String>), Summary }
impl Ask {
    /// The tool's arguments, as the Rails tool hands them to its service.
    fn tool(name:&str,args:&Value)->Option<Self>{
        Some(match name {
            "get_bot_details"=>Self::Details(args["bot_id"].as_f64().unwrap_or(0.0) as i64),
            "get_exchange_balances"=>Self::Balances{name:args["exchange_name"].as_str().unwrap_or("").into(),id:None},
            "list_open_orders"=>Self::Orders(args["exchange_name"].as_str().filter(|s|!s.trim().is_empty()).map(str::to_string)),
            "get_portfolio_summary"=>Self::Summary,
            _=>return None,
        })
    }
}
/// Planned: answered from the database alone, or a venue fetch to run outside it.
pub enum Planned { Done(Read), Fetch(Fetch) }
fn credentials(c:&Connection,app:&App,user:i64,exchange:i64)->Result<Option<Material>,WebError>{
    let id=c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 AND status=1 LIMIT 1",[user,exchange],|r|r.get(0)).optional()?;
    id.map(|id|sync::credentials_with_version(c,&app.cipher,id).map(|(credentials,origin)|Material{credentials,origin}).map_err(|_|error())).transpose()
}
const EXCHANGE:&str="SELECT id,name,type FROM exchanges WHERE type!='Exchanges::Bitmart'";
fn exchange(c:&Connection,name:&str)->Result<Option<(i64,String,String)>,WebError>{Ok(c.query_row(&format!("{EXCHANGE} AND lower(name)=?1 LIMIT 1"),[name.to_lowercase()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)}
/// MCP: the tool's plan, as a finished answer or a fetch.
pub fn plan(c:&Connection,app:&App,user:i64,name:&str,args:&Value)->Result<Called,WebError>{
    let ask=Ask::tool(name,args).ok_or_else(error)?;
    Ok(match plan_ask(c,app,user,ask)? {Planned::Done(read)=>done(read),Planned::Fetch(fetch)=>Called::Fetch(fetch)})
}
/// Either surface: the read planned in one transaction; a fetch carries the credential versions it was planned for.
pub fn plan_ask(c:&Connection,app:&App,user:i64,ask:Ask)->Result<Planned,WebError>{
    let tx=c.unchecked_transaction()?;
    let planned=plan_inner(&tx,app,user,ask)?;
    let out=match planned {
        Planned::Fetch(inner)=>{
            let versions=inner.producers();
            Planned::Fetch(Fetch::Bound{inner:Box::new(inner),versions})
        }
        done=>done,
    };
    tx.commit()?;Ok(out)
}
fn plan_inner(c:&Connection,app:&App,user:i64,ask:Ask)->Result<Planned,WebError>{
    if !read_limits::check(c,user)?{return Ok(Planned::Done(unported(read_limits::REFUSAL,false)))}
    let now=At::from_utc(app.now()).ok_or_else(error)?;
    match ask {
        Ask::Details(id)=>Ok(Planned::Done(budget::within(||details(c,user,id,now))?)),
        Ask::Balances{name,id}=>{
            // BotApi::Exchanges::Balances#resolve_exchange: by id when one is given (`to_i`), else by name.
            let found=match &id {
                Some(id)=>c.query_row(&format!("{EXCHANGE} AND id=?1"),[crate::web::bot_api::to_i(&json!(id)).unwrap_or(0)],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,String>(2)?))).optional()?,
                None=>exchange(c,&name)?,
            };
            let Some((exchange,name,kind))=found else{
                let shown=if name.trim().is_empty(){id.unwrap_or_default()}else{name};
                return Ok(Planned::Done(failed(Domain::NotFound,"exchange_not_found",format!("Exchange '{shown}' not found. Available exchanges: {}",super::tradeable(c)?.join(", ")))))
            };
            let Some(credentials)=credentials(c,app,user,exchange)? else{return Ok(Planned::Done(failed(Domain::PermissionDenied,"api_key_missing",format!("No valid API key found for {name}. Please add an API key in Settings."))))};
            if kind!="Exchanges::Alpaca" {return Ok(Planned::Done(unported(ONLY,true)))} // Rails reads the venue; a failed read is a tool error.
            Ok(Planned::Fetch(Fetch::Balances{user,exchange,name,credentials}))
        },
        Ask::Orders(filter)=>{
            let filter=if let Some(name)=filter {
                match exchange(c,&name)?{Some(ex)=>Some(ex),None=>return Ok(Planned::Done(failed(Domain::NotFound,"exchange_not_found",format!("Exchange '{name}' not found. Available: {}",super::tradeable(c)?.join(", ")))))}
            }else{None};
            let Some((local,rows,ids))=local_orders(c,user,filter.as_ref().map(|x|x.0))? else{return Ok(Planned::Done(unported(read_limits::REFUSAL,false)))};
            let exchanges=if let Some(ex)=filter {vec![ex]}else{
                c.prepare("SELECT DISTINCT e.id,e.name,e.type FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0 AND k.status=1 ORDER BY k.id")?.query_map([user],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?
            };
            // Only Exchanges::Alpaca lists open orders (`respond_to?(:list_open_orders)`).
            let mut venues=vec![];
            for (id,name,kind) in exchanges {if kind=="Exchanges::Alpaca" {venues.push((id,name,credentials(c,app,user,id)?));}}
            Ok(Planned::Fetch(Fetch::Orders{user,local,rows,ids,venues}))
        },
        Ask::Summary=>{
            let n:i64=c.query_row("SELECT count(*) FROM bots WHERE user_id=?1 AND status!=3",[user],|r|r.get(0))?;
            if n==0 {return Ok(Planned::Done(ok(json!({"empty":true,"bots":[],"totals":null,"global_pnl":null}),"No bots found. Create a bot to start tracking your portfolio.")))}
            let refused=budget::within(||->Result<bool,WebError>{
                for id in ids(c,user)?{
                    let Some(b)=bot(c,user,id)? else{continue};
                    let (subject,metrics)=match metrics(c,&b,now){Ok(v)=>v,Err(WebError::Config(why)) if why=="executed fill value unavailable"=>continue,Err(e)=>return Err(e)};
                    let needs_price=if pair_kind(&b.kind){!metrics.chart.labels.is_empty()&&!subject.tickers.is_empty()}else{live::needs_venue_price(c,&subject,&metrics,now).map_err(fail_fig)?};
                    if subject.bot.exchange_type.as_deref()!=Some("Exchanges::Alpaca")&&needs_price{return Ok(true)}
                }Ok(false)
            })?;
            if refused{return Ok(Planned::Done(unported(ONLY,false)))}
            let info=figure::loading::read_info(c,app,user)?;
            let credentials=match (info.credentials,info.producer.alpaca()) { (Some(credentials),Some(origin))=>Some(Material{credentials,origin:origin.clone()}), _=>None };
            Ok(Planned::Fetch(Fetch::Summary{user,credentials,producer:info.producer,now}))
        },
    }
}
/// An account read (the balances, the open orders) is a key check's kind of request: sent live, to the hosts the key
/// checks use (a test points them at its own server). A scripted source answers it instead, by Alpaca's own hosts.
fn account_venue(app:&App,credentials:&Material)->AlpacaVenue<Wire>{
    let (source,hosts)=match &app.figure_source {
        Source::Script(..)|Source::Counted(..)=>(app.figure_source.clone(),Hosts::alpaca()),
        Source::Live|Source::Disabled=>(Source::Live,app.settings_key_hosts.clone()),
    };
    AlpacaVenue::new(Wire::new(credentials,source).with_origin(credentials.origin.clone()),Urls::pick(credentials.passphrase.as_deref(),&hosts))
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
            let venue=account_venue(app,&credentials);
            match balance_read(&venue).await {
                Ok((cash,held))=>Fetched::Balances(user,exchange,name,cash,held),
                Err((_,true))=>Fetched::Done(raised()),
                Err((why,false))=>Fetched::Done(failed(Domain::UpstreamFailed,"balances_fetch_failed",format!("Failed to fetch balances from {name}: {}",sync::scrub(&why,&credentials)))),
            }
        },
        Fetch::Orders{user,local,rows,ids,venues}=>{
            let mut out=vec![];
            for (id,name,credentials) in venues {
                let Some(credentials)=credentials else{out.push((id,name,Err("no usable trading key".into())));continue};
                let venue=account_venue(app,&credentials);
                let result=match venue.read(false,"/v2/orders",vec![("status","open".into()),("limit","50".into())],sync::balances::MAX_LIST_BYTES).await {
                    Err(e)=>Err(sync::venue_failure(e)),
                    Ok(body)=>sync::parsed(body,|text|serde_json::from_str::<RawOrders>(text).map_err(|_|sync::Unread::Raised("Unreadable orders".into()))).await
                };
                match result {Ok(v)=>out.push((id,name,Ok(v))),Err((_,true))=>return Fetched::Done(raised()),Err((why,false))=>out.push((id,name,Err(sync::scrub(&why,&credentials))))}
            }
            Fetched::Orders(user,local,rows,ids,out)
        },
        Fetch::Summary{user,credentials,producer,now}=>{
            let mut cache=Cache::default();
            cache.produced_by(producer);
            // Without a usable Alpaca key the other venues are read, and Alpaca's market data is a settled failure.
            let wire=match &credentials { Some(credentials)=>Wire::new(credentials,app.figure_source.clone()).with_origin(credentials.origin.clone()), None=>Wire::of(None,app.figure_source.clone()) };
            let ready=figure::loading::fill_with(app,user,&mut cache,&wire,now,read_limits::check,move|c,r|global(c,user,r,now).is_ok()).await;
            Fetched::Summary(user,cache,now,ready)
        }
    }
}
/// MCP: the tool's answer for a finished fetch.
pub fn finish(c:&Connection,fetch:Fetched)->Result<Value,WebError>{Ok(tool_result(&finish_read(c,fetch)?))}
/// Either surface: a finished fetch's answer, from one SQLite snapshot.
pub fn finish_read(c:&Connection,fetch:Fetched)->Result<Read,WebError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=finish_in(&tx,fetch)?;tx.commit()?;Ok(out)}else{finish_in(c,fetch)}
}
/// Every envelope and derived reply consumes the same finishing SQLite snapshot.
fn finish_in(c:&Connection,fetch:Fetched)->Result<Read,WebError>{
    if let Fetched::Bound(inner,versions)=fetch {
        for version in versions {
            if !crate::engine::model::credential_is_current(c,&version)? {
                crate::engine::log(crate::engine::model::CREDENTIALS_CHANGED);
                return Ok(unported("Figures unavailable: credentials changed; retry with current credentials",false));
            }
        }
        return finish_in(c,*inner)
    }

    let user=match &fetch{Fetched::Balances(u,..)|Fetched::Orders(u,..)|Fetched::Summary(u,..)=>Some(*u),Fetched::Done(..)|Fetched::Bound(..)=>None};
    if let Some(user)=user{if !read_limits::check(c,user)?{return Ok(unported(read_limits::REFUSAL,false))}}
    match fetch{
        Fetched::Bound(..)=>Err(error()),
        Fetched::Done(read)=>Ok(read),
        Fetched::Balances(user,exchange,name,cash,held)=>{
            let catalog=sync::balances::catalog_for(c,user,exchange).map_err(|_|error())?;
            let mut lines=vec![];let mut rows=vec![];
            for (id,free,locked) in match sync::balances::complete_balances(&catalog,cash,held){Ok(rows)=>rows,Err(why)=>return Ok(failed(Domain::UpstreamFailed,"balances_fetch_failed",format!("Failed to fetch balances from {name}: {why}")))}{
                let (n,l)=(free.to_f(),locked.to_f()); if n==0.0&&l==0.0{continue}
                let symbol:Option<String>=c.query_row("SELECT symbol FROM assets WHERE id=?1",[id],|r|r.get(0)).optional()?.flatten();
                let symbol=symbol.unwrap_or_else(||format!("Unknown({id})"));
                let shown=if l>0.0{format!(" ({} locked)",float_to_s(l))}else{String::new()};
                lines.push(format!("- {symbol}: {}{shown}",float_to_s(n)));
                rows.push(json!({"asset_id":id,"symbol":symbol,"free":n,"locked":l}));
            }
            let text=if lines.is_empty(){format!("All balances on {name} are zero.")}else{format!("{name} Balances:\n{}",lines.join("\n"))};
            Ok(ok(json!({"exchange":name,"count":rows.len(),"balances":rows}),text))
        },
        Fetched::Orders(_user,mut local,mut rows,ids,venues)=>{
            let mut unavailable=vec![];let mut missing=vec![];
            for (exchange,name,result) in venues{
                match result{
                    Err(why)=>{unavailable.push(format!("! {name}: could not be checked ({why}) — open orders there are not listed"));missing.push(json!({"exchange":name,"error":why}));}
                    Ok(listed)=>{
                        if !read_limits::count(listed.len(),read_limits::VENUE_ORDERS){return Ok(unported(read_limits::REFUSAL,false))}
                        for raw in listed{
                        let (id,pair,class)=crate::venue::alpaca::order_identity(raw.get());
                        let id=id.as_deref().unwrap_or("");if ids.contains(id){continue}
                        let Some(ticker)=crate::engine::model::alpaca_order_ticker(c,exchange,pair.as_deref(),class.as_deref()).map_err(|_|error())? else {
                            eprintln!("Alpaca order skipped: unsupported class or unmapped/ambiguous order identity");
                            continue;
                        };
                        let raw=crate::venue::http::decode_json(raw.get()).map_err(|_|error())?;
                        let parsed=crate::venue::alpaca::parse_read_order(id,&raw).map_err(|_|error())?;
                        let pair=format!("{}/{}",ticker.base_code,ticker.quote_code);
                        let amount=parsed.amount.as_ref().map(|a|a.to_s_f());
                        let price=parsed.price.as_ref().map(|p|format!("@ {}",p.to_s_f())).unwrap_or_default(); // Rails omits the price only when nil.
                        let side=raw["side"].as_str().map(str::to_uppercase).unwrap_or_else(||"?".into());
                        local.push(format!("- {side} {} {pair} {price} ({}) | {name} | ext: {id}",amount.clone().unwrap_or_else(||"N/A".into()),if parsed.limit{"Limit order"}else{"Market order"}));
                        // parse_order_data's price: the fill price when positive, else the limit price, else the Integer 0.
                        rows.push(json!({"source":"exchange","external_id":id,"exchange":name,"side":raw["side"].as_str(),"order_type":if parsed.limit{"limit_order"}else{"market_order"},
                                         "pair":pair,"amount":amount,"price":parsed.price.map_or(json!(0),|p|json!(p.to_s_f()))}));
                    }}
                }
            }
            let text=if local.is_empty()&&unavailable.is_empty(){"No open orders found.".into()}else{
                let header=if local.is_empty(){"No open orders found on the exchanges that answered.".into()}else{format!("Open orders ({}):",local.len())};
                [vec![header],local,unavailable].concat().join("\n")
            };
            Ok(ok(json!({"count":rows.len(),"orders":rows,"unavailable":missing}),text))
        },
        Fetched::Summary(user,cache,now,ready)=>budget::within(||summary(c,user,&cache,now,ready)),
    }
}
/// The bot-managed open orders: the tool's lines, the service's rows and the external ids they cover. `None` past the
/// read bound.
#[allow(clippy::type_complexity)]
fn local_orders(c:&Connection,user:i64,exchange:Option<i64>)->Result<Option<(Vec<String>,Vec<Value>,HashSet<String>)>,WebError>{
    let n:usize=c.query_row("SELECT count(*) FROM (SELECT t.id FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND t.status=0 AND t.external_status=1 AND (?2 IS NULL OR t.exchange_id=?2) LIMIT 101)",rusqlite::params![user,exchange],|r|r.get(0))?;
    if !read_limits::count(n,read_limits::LOCAL_ORDERS){return Ok(None)}
    let zone:String=c.query_row("SELECT time_zone FROM users WHERE id=?1",[user],|r|r.get(0))?;
    let mut q=c.prepare("SELECT t.id,t.created_at,t.side,t.base,t.quote,t.order_type,e.name,t.external_id FROM transactions t JOIN bots b ON b.id=t.bot_id JOIN exchanges e ON e.id=t.exchange_id WHERE b.user_id=?1 AND t.status=0 AND t.external_status=1 AND (?2 IS NULL OR t.exchange_id=?2) ORDER BY t.created_at DESC LIMIT 100")?;
    let rows=q.query_map(rusqlite::params![user,exchange],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,String>(6)?,r.get::<_,Option<String>>(7)?)))?;
    let mut lines=vec![];let mut out=vec![];let mut ids=HashSet::new();
    for row in rows{
        let (id,time,side,base,quote,kind,name,ext)=row?;
        let [price,amount,_,_,_]=crate::figures::fill::display_amounts(c,id)?;
        let at=crate::codec::parse_time(&time).map_err(|_|error())?;
        let side=match side{Some(0)=>"buy",Some(1)=>"sell",_=>""};
        let order_type=match kind{Some(0)=>Some("market_order"),Some(1)=>Some("limit_order"),_=>None};
        let pair=format!("{}/{}",base.unwrap_or_default(),quote.unwrap_or_default());
        out.push(json!({"source":"db","id":id,"external_id":ext,"exchange":name,"side":side,"order_type":order_type,"pair":pair,
                        "amount":amount.as_ref().map(|n|n.to_s_f()),"price":price.as_ref().map(|n|n.to_s_f()),"created_at":crate::ruby::iso8601_ms(at)}));
        let amount=amount.map(|n|n.to_s_f());
        let price=price.filter(|n|!n.is_zero()).map(|n|n.to_s_f());
        let date=crate::web::timezone::local(at,&zone).format("%Y-%m-%d %H:%M");
        let ext=ext.unwrap_or_default(); // nil interpolates to the empty string in Rails.
        if !ext.trim().is_empty(){ids.insert(ext.clone());}
        // Optional DB columns interpolate as empty; an absent amount alone prints N/A.
        lines.push(format!("- [{date}] #{id} {} {} {pair} {} ({}) | {name} | ext: {ext}",side.to_uppercase(),amount.unwrap_or_else(||"N/A".into()),price.map(|v|format!("@ {v}")).unwrap_or_default(),match order_type{Some("market_order")=>"Market order",Some(_)=>"Limit order",None=>"Unknown"}));
    }Ok(Some((lines,out,ids)))
}
fn num(n:&Num)->Result<String,WebError>{
    Ok(match n{Num::Int(i)=>i.to_string(),Num::Float(f)=>float_to_s(sync::balances::float_round(*f,2)),Num::Dec(d)=>d.round(2).map_err(|_|error())?.to_s_f()})
}
fn pct(n:&Num)->Result<String,WebError>{Ok(format!("{}{}%",if n.is_negative(){""}else{"+"},num(&n.mul(&Num::Int(100)).map_err(|_|error())?)?))}
fn fail_fig(e:figures::FiguresError)->WebError{match e{figures::FiguresError::Data(ref why)|figures::FiguresError::NotComputed(ref why) if why=="executed fill value unavailable"||why=="quote currency unavailable"=>WebError::Config(why.clone()),_=>error()}}
struct Bot {id:i64,label:String,kind:String,status:i64,settings:Value,transient:Value,exchange:Option<String>,quote_asset:Option<String>,quote:String,base:Option<String>,pair:Option<String>,started:Option<String>}
fn bot(c:&Connection,user:i64,id:i64)->Result<Option<Bot>,WebError>{
    read_limits::charge_bot()?;
    let row=c.query_row("SELECT b.label,b.type,b.status,b.settings,e.name,b.started_at,b.transient_data FROM bots b LEFT JOIN exchanges e ON e.id=b.exchange_id WHERE b.id=?1 AND b.user_id=?2 AND b.status!=3",[id,user],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?))).optional()?;
    let Some((label,kind,status,settings,exchange,started,transient))=row else{return Ok(None)};
    crate::figures::fill::validate_row_times(c,id).map_err(fail_fig)?;
    let settings:Value=serde_json::from_str(&settings).map_err(|_|error())?;
    let transient:Value=serde_json::from_str(&transient).map_err(|_|error())?;
    crate::codec::validate_bot_times(&settings,&transient).map_err(|_|error())?;
    started.as_deref().map(crate::codec::parse_time).transpose().map_err(|_|error())?;
    let changed:Option<String>=c.query_row("SELECT settings_changed_at FROM bots WHERE id=?1",[id],|r|r.get(0))?;
    changed.as_deref().map(crate::codec::parse_time).transpose().map_err(|_|error())?;
    let quote_asset=tools::asset(c,&settings["quote_asset_id"])?;
    let quote=quote_asset.clone().unwrap_or_default(); // nil quote interpolates as empty.
    let members=members(c,&kind,&settings)?;
    let base=if pair_kind(&kind){tools::asset(c,&settings["base_asset_id"])?}else if members.len()==1{Some(members[0].1.clone())}else{None};
    let pair=if kind=="Bots::DcaMultiAsset"{Some(format!("{}/{}",members.iter().map(|(_,s,_)|s.as_str()).collect::<Vec<_>>().join("+"),quote))}else if pair_kind(&kind){base.as_ref().filter(|_|!quote.is_empty()).map(|base|format!("{base}/{quote}"))}else{None};
    let label=match label.filter(|s|!s.trim().is_empty()){
        Some(label)=>label,
        None=>generated_label(c,user,id,&kind,&settings,"en")?,
    };
    Ok(Some(Bot{id,label,kind,status,settings,transient,exchange,quote_asset,quote,base,pair,started}))
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
fn effective_started(b:&Bot)->Result<Option<chrono::DateTime<chrono::Utc>>,WebError>{
    let Some(raw)=&b.started else{return Ok(None)};
    let mut start=crate::codec::parse_time(raw).map_err(|_|error())?;
    // Signal and index have no trigger decorators.
    if matches!(b.kind.as_str(),"Bots::Signal"|"Bots::DcaIndex") {return Ok(Some(start))}
    let selling=b.settings["direction"]=="selling";
    let prefix=if selling{"sell_"}else{""};
    for trigger in ["price","price_drop","moving_average","indicator"]{
        if b.settings[format!("{prefix}{trigger}_limited")].as_bool()!=Some(true){continue}
        let Some(raw)=b.transient[format!("{prefix}{trigger}_limit_condition_met_at")].as_str().filter(|s|!s.trim().is_empty()) else{return Ok(None)};
        // R9: stored SQL timestamps have the engine's UTC meaning in every adapter.
        let met=crate::codec::parse_time(raw).map_err(|_|error())?;
        start=start.max(met);
    }
    Ok(Some(start))
}
/// A figure as Rails renders it: an Integer, a Float, a BigDecimal as its text.
fn number(n:&Num)->Value{match n{Num::Int(i)=>json!(i),Num::Float(f)=>json!(f),Num::Dec(d)=>json!(d.to_s_f())}}
/// A metrics failure's message as the service carries it.
fn metrics_error(e:&WebError)->&'static str{match e{WebError::Config(why) if why=="executed fill value unavailable"=>"executed fill value unavailable",_=>"metrics unavailable"}}
/// BotApi::Bots::Get, and GetBotDetailsTool's text for it.
fn details(c:&Connection,user:i64,id:i64,now:At)->Result<Read,WebError>{
    let Some(b)=bot(c,user,id)?else{return Ok(failed(Domain::NotFound,"bot_not_found","Bot not found."))};
    let status=tools::STATUSES.get(usize::try_from(b.status).map_err(|_|error())?).ok_or_else(error)?;
    let mut lines=vec![format!("Bot: {} (ID {})",b.label,b.id),format!("Type: {}",tools::type_name(&b.kind)),format!("Status: {status}"),format!("Exchange: {}",b.exchange.as_deref().unwrap_or("N/A"))];
    if let Some(pair)=&b.pair{lines.push(format!("Pair: {pair}"));}
    let members=members(c,&b.kind,&b.settings)?;
    // allocation_assets: a basket's members, each under the key it goes by (nil for any other type).
    let mut assets=(b.kind=="Bots::DcaMultiAsset").then(Vec::new);
    if !members.is_empty(){
        let names=figures::db::asset_names(c,&members.iter().map(|(id,_,_)|*id).collect::<Vec<_>>()).map_err(fail_fig)?;
        let candidates=names.iter().map(|(id,symbol,name)|(figures::keys::Identity::Asset(*id),figures::keys::candidate(*id,symbol.as_deref(),name.as_deref()))).collect::<Vec<_>>();
        let keys=figures::keys::call(&candidates).map_err(|_|error())?;
        let mut formatted=vec![];let mut listed=vec![];
        for (id,symbol,w) in &members {
            let key=keys.iter().find(|(asset,_)|*asset==figures::keys::Identity::Asset(*id)).map(|(_,key)|key).ok_or_else(error)?;
            formatted.push(format!("{key} {:.2}%",w*100.0));
            // The ids update_bot_settings takes back: the asset, and the ticker a trading condition watches.
            let ticker:Option<i64>=c.query_row("SELECT t.id FROM bot_index_assets m JOIN tickers t ON t.id=m.ticker_id WHERE m.bot_id=?1 AND m.in_index=1 AND t.base_asset_id=?2 ORDER BY m.id DESC LIMIT 1",(b.id,*id),|r|r.get(0)).optional()?;
            listed.push(format!("{key} (asset id {id}, ticker id {})",ticker.map_or_else(||"none".to_string(),|t|t.to_string())));
            let name=names.iter().find(|(asset,_,_)|asset==id).and_then(|(_,_,name)|name.clone());
            if let Some(assets)=assets.as_mut(){assets.push(json!({"key":key,"asset_id":id,"ticker_id":ticker,"symbol":symbol,"name":name,"weight":w}));}
        }
        lines.push(format!("Allocations: {}",formatted.join(", ")));
        lines.push(format!("Members: {}",listed.join(", ")));
    }
    let result=metrics(c,&b,now);
    let mut redeploy_line=None;
    // held_symbols, exited_symbols, redeploy_offer: composition bots only; nil where their metrics fail.
    let (mut holdings,mut exited_holdings,mut redeploy)=(Value::Null,Value::Null,Value::Null);
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
        (holdings,exited_holdings)=(json!(held),json!(exited));
        if figures::db::composition_minimum(c,s).map_err(fail_fig)?.is_none(){
            redeploy_line=Some("Redeploy unavailable: no composition minimum".into());
        }else{
            let offer=figure::redeploy_offer(c,s,m,&in_index).map_err(fail_fig)?.filter(|v|v.is_positive());
            redeploy=json!(offer.as_ref().map_or_else(||"0".to_string(),|o|o.to_s_f()));
            if let Some(offer)=offer{redeploy_line=Some(format!("Redeploy offer (answer with answer_redeploy_offer): {} {}",offer.to_s_f(),b.quote));}
        }
    }
    }
    let (enabled,jurisdiction,zone):(Option<bool>,Option<String>,String)=c.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction,time_zone FROM users WHERE id=?1",[user],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let jurisdiction=jurisdiction.as_deref().filter(|s|!s.trim().is_empty()).unwrap_or("US");
    let mut locked=vec![];
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
                if until>now.utc(){
                    let days=(crate::web::timezone::local(until,&zone).date_naive()-crate::web::timezone::local(now.utc(),&zone).date_naive()).num_days();
                    locks.push(format!("{} {days}d",symbol.clone().unwrap_or_default())); // nil symbol interpolates empty, as Rails.
                    // The service's days are counted in REST's zone, UTC.
                    locked.push(json!({"symbol":symbol,"days_left":(until.date_naive()-now.utc().date_naive()).num_days()}));
                }
            }
        }
        if !locks.is_empty(){lines.push(format!("Locked out of buying (wash sale): {}",locks.join(", ")));}
    }
    if let Some(line)=redeploy_line{lines.push(line);}
    lines.push(format!("Interval: {}",b.settings["interval"].as_str().unwrap_or("N/A")));
    lines.push(format!("Amount per order: {} {}",tools::str_value(&b.settings["quote_amount"]),b.quote));
    let count:i64=c.query_row("SELECT count(*) FROM transactions WHERE bot_id=?1 AND status IN (0,2)",[id],|r|r.get(0))?;
    lines.push(format!("Orders executed: {count}"));
    let started=effective_started(&b)?;
    if let Some(start)=started{
        lines.push(format!("Started: {}",crate::web::timezone::local(start,&zone).format("%Y-%m-%d %H:%M %Z")));
    }
    lines.push(String::new());
    let (mut shown_metrics,mut shown_error)=(Value::Null,Value::Null);
    match result{
        Err(e)=>{let why=metrics_error(&e);lines.push(format!("Metrics unavailable: {why}"));shown_error=json!(why);}
        Ok((_,m))=>{
            lines.push("--- Performance ---".into());
            lines.push(format!("Total invested: {} {}",num(&m.total_quote_amount_invested)?,b.quote));
            lines.push(format!("Current value: {} {}",num(&m.total_amount_value_in_quote)?,b.quote));
            if let Some(p)=&m.pnl{lines.push(format!("P/L: {}",pct(p)?));}
            // #holding: a pair bot's own figures; a one-asset basket's from its member's row; none for the others.
            let (mut average,mut amount)=(Value::Null,Value::Null);
            if pair_kind(&b.kind){
                let p=figures::pair::metrics(c,b.id,now).map_err(fail_fig)?;
                if let Some(avg)=&p.average{lines.push(format!("Average buy price: {} {}",num(avg)?,b.quote));average=number(avg);}
                lines.push(format!("Total acquired: {} {}",num(&p.amount)?,b.base.as_deref().unwrap_or("units")));
                amount=number(&p.amount);
            }else if let Some(base)=&b.base{
                let identity=members.first().map(|m|m.0);
                let key=m.key_assets.iter().find(|(_,asset)|*asset==identity).map(|(key,_)|key);
                let row=m.asset_breakdown.iter().find(|(k,_)|Some(k)==key);
                let held=row.map(|(_,r)|r.amount.clone()).unwrap_or(Num::Int(0));
                if let Some((_,r))=row.filter(|(_,r)|r.amount.is_positive()) {lines.push(format!("Average buy price: {} {}",num(&r.quote_invested.div(&r.amount).map_err(|_|error())?)?,b.quote));}
                lines.push(format!("Total acquired: {} {base}",num(&held)?));
                // `row[:amount].to_d`, and `row[:quote_invested].to_d / amount` while it is positive.
                let d=held.to_d().map_err(|_|error())?;
                if d.is_positive(){let r=&row.ok_or_else(error)?.1;average=json!(r.quote_invested.to_d().map_err(|_|error())?.div(&d).map_err(|_|error())?.to_s_f());}
                amount=json!(d.to_s_f());
            }
            shown_metrics=json!({"invested":number(&m.total_quote_amount_invested),"value":number(&m.total_amount_value_in_quote),"pnl":m.pnl.as_ref().map_or(Value::Null,number),
                                 "average_buy_price":average,"total_base_amount":amount});
        }
    }
    let allocations=assets.as_ref().map(|a|a.iter().map(|m|(m["key"].as_str().unwrap_or("").to_string(),m["weight"].clone())).collect::<serde_json::Map<_,_>>());
    let data=json!({"id":b.id,"label":b.label,"type":b.kind,"status":status,"exchange":b.exchange,"pair":b.pair,"allocations":allocations,"allocation_assets":assets,
                    "holdings":holdings,"exited_holdings":exited_holdings,"locked_members":locked,"redeploy_offer":redeploy,"base_asset":b.base,"quote_asset":b.quote_asset,
                    "interval":b.settings.get("interval").cloned().unwrap_or(Value::Null),"quote_amount":b.settings.get("quote_amount").cloned().unwrap_or(Value::Null),
                    "orders_executed":count,"started_at":started.map(crate::ruby::iso8601_ms),"metrics":shown_metrics,"metrics_error":shown_error});
    Ok(ok(data,lines.join("\n")))
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
/// BotApi::Portfolio::Summary, and GetPortfolioSummaryTool's text for it.
fn summary(c:&Connection,user:i64,cache:&Cache,now:At,ready:bool)->Result<Read,WebError>{
    let bots=ids(c,user)?.into_iter().map(|id|bot(c,user,id)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten().collect::<Vec<_>>();
    let count=|statuses:&[i64]|bots.iter().filter(|b|statuses.contains(&b.status)).count();
    let archived=count(&[7]);
    let totals=json!({"total":bots.len(),"working":count(&[1,4,5,6]),"stopped":count(&[2]),"created":count(&[0]),"archived":archived});
    let mut lines=vec!["Portfolio Summary".into(),"================".into(),format!("Total bots: {} ({} active, {} stopped, {} not started{})",bots.len(),count(&[1,4,5,6]),count(&[2]),count(&[0]),if archived>0{format!(", {archived} archived")}else{String::new()}),String::new()];
    let reader=Reader::new(cache,now.utc().timestamp()).with_current(c).with_symbols(figure::loading::symbols(c,user).map_err(fail_fig)?);
    let reason=if bots.iter().any(|b|b.quote.trim().is_empty()){Some("quote currency unavailable")}
        else if bots.iter().any(|b|matches!(metrics(c,b,now),Err(WebError::Config(why)) if why=="executed fill value unavailable")){Some("executed fill value unavailable")}
        else{None};
    let pnl=if ready&&reason.is_none(){global(c,user,&reader,now)?}else{None};
    let global_pnl=pnl.as_ref().map_or(Value::Null,|p|json!({"percent":number(&p.percent),"profit_usd":number(&p.profit_usd)}));
    if let Some(p)=pnl{
        lines.push(format!("Global P/L: {}",pct(&p.percent)?));
        lines.push(format!("Profit (USD): {}${}",if p.profit_usd.is_negative(){""}else{"+"},num(&p.profit_usd)?));
    }else if let Some(why)=reason{
        lines.push(format!("Global P/L: Not available ({why})"));
        if why=="quote currency unavailable"{lines.push(format!("Profit (USD): Not available ({why})"));}
    }else{lines.push("Global P/L: Not available (needs market data)".into());}
    lines.extend([String::new(),"--- Per-Bot Summary ---".into()]);
    let mut rows=vec![];
    for b in bots{
        let status=tools::STATUSES.get(usize::try_from(b.status).map_err(|_|error())?).ok_or_else(error)?;
        let name=format!("- {} (ID {}) ({}) | {status} | ",b.label,b.id,b.pair.as_deref().unwrap_or("N/A"));
        let (shown,why)=match metrics(c,&b,now){
            Ok((_,m))=>if let Some(p)=&m.pnl{
                lines.push(format!("{name}P/L: {} | Invested: {} {}",pct(p)?,num(&m.total_quote_amount_invested)?,b.quote));
                (json!({"pnl":number(p),"invested":number(&m.total_quote_amount_invested)}),Value::Null)
            }else{lines.push(format!("{name}No metrics yet"));(Value::Null,Value::Null)},
            Err(e)=>{
                let why=metrics_error(&e);
                lines.push(if why=="executed fill value unavailable"{format!("{name}Metrics unavailable: executed fill value unavailable")}else{format!("{name}Metrics unavailable")});
                (Value::Null,json!(why))
            }
        };
        rows.push(json!({"id":b.id,"label":b.label,"status":status,"pair":b.pair,"quote_asset":b.quote_asset,"metrics":shown,"metrics_error":why}));
    }
    Ok(ok(json!({"empty":false,"totals":totals,"global_pnl":global_pnl,"bots":rows}),lines.join("\n")))
}
