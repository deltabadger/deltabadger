//! The four read tools. Network work runs outside App::db; accounting uses the merged figures library.
use super::{protocol::tool_text,read_limits,tools::{self,Called}};
use crate::{crypto::Credentials,figures::num::float_to_s,sync,venue::alpaca::{AlpacaVenue,Urls},web::{App,WebError,figure::loading::Wire}};
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;
pub const NAMES:[&str;2]=["get_exchange_balances","list_open_orders"];
const ONLY:&str="this build reads Alpaca only";
fn error()->WebError{WebError::Config("MCP read unavailable".into())}
fn done(text:impl AsRef<str>)->Called{Called::Done(tool_text(read_limits::text(text.as_ref()),false))}
pub enum Fetch {
    Balances{user:i64,exchange:i64,name:String,credentials:Credentials},
    Orders{user:i64,local:Vec<String>,ids:HashSet<String>,venues:Vec<(i64,String,Option<Credentials>)>},
}
pub enum Fetched { Text(String,bool), Balances(i64,i64,String,crate::ruby::BigDec,Vec<(String,crate::ruby::BigDec)>), Orders(i64,Vec<String>,HashSet<String>,Vec<(i64,String,Result<Value,String>)>) }
fn credentials(c:&Connection,app:&App,user:i64,exchange:i64)->Result<Option<Credentials>,WebError>{
    let id=c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 AND status=1 LIMIT 1",[user,exchange],|r|r.get(0)).optional()?;
    id.map(|id|sync::credentials(c,&app.cipher,id).map_err(|_|error())).transpose()
}
fn exchange(c:&Connection,name:&str)->Result<Option<(i64,String,String)>,WebError>{Ok(c.query_row("SELECT id,name,type FROM exchanges WHERE lower(name)=?1 AND type!='Exchanges::Bitmart' LIMIT 1",[name.to_lowercase()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)}
fn unknown(c:&Connection,name:&str)->Result<Called,WebError>{Ok(done(format!("Exchange '{name}' not found. Available exchanges: {}",super::tradeable(c)?.join(", "))))}
pub fn plan(c:&Connection,app:&App,user:i64,name:&str,args:&Value)->Result<Called,WebError>{
    if !read_limits::check(c,user)?{return Ok(done(read_limits::REFUSAL))}
    match name {
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
        _=>Err(error())
    }
}
async fn balance_read(venue:&AlpacaVenue<Wire>)->Result<(crate::ruby::BigDec,Vec<(String,crate::ruby::BigDec)>),(String,bool)>{
    let cash=sync::parsed(venue.read(false,"/v2/account",vec![],sync::balances::MAX_ACCOUNT_BYTES).await.map_err(sync::venue_failure)?,sync::balances::account).await?;
    let held=sync::parsed(venue.read(false,"/v2/positions",vec![],sync::balances::MAX_LIST_BYTES).await.map_err(sync::venue_failure)?,sync::balances::positions).await?;
    match (cash,held){(Ok(c),Ok(h))=>Ok((c,h)),(Err(e),_)|(_,Err(e))=>Err((e,false))}
}
pub async fn fetch(app:&App,fetch:Fetch)->Fetched{
    match fetch{
        Fetch::Balances{user,exchange,name,credentials}=>{
            let venue=AlpacaVenue::new(Wire::new(&credentials,app.figure_source.clone()),Urls::for_passphrase(credentials.passphrase.as_deref()));
            match balance_read(&venue).await {Ok((cash,held))=>Fetched::Balances(user,exchange,name,cash,held),Err((_,true))=>Fetched::Text("An unexpected error occurred.".into(),true),Err((why,false))=>Fetched::Text(format!("Failed to fetch balances from {name}: {}",sync::scrub(&why,&credentials)),false)}
        },
        Fetch::Orders{user,local,ids,venues}=>{
            let mut out=vec![];
            for (id,name,credentials) in venues {
                let Some(credentials)=credentials else{out.push((id,name,Err("no usable trading key".into())));continue};
                if credentials.passphrase.as_deref()==Some("live"){out.push((id,name,Err("this build reads Alpaca paper only".into())));continue}
                let venue=AlpacaVenue::new(Wire::new(&credentials,app.figure_source.clone()),Urls::for_passphrase(None));
                let result=match venue.read(false,"/v2/orders",vec![("status","open".into()),("limit","50".into())],sync::balances::MAX_LIST_BYTES).await {
                    Err(e)=>Err(sync::venue_failure(e)),
                    Ok(body)=>sync::parsed(body,|text|crate::venue::http::decode_json(text).map_err(|_|sync::Unread::Raised("Unreadable orders".into()))).await
                };
                match result {Ok(v)=>out.push((id,name,Ok(v))),Err((_,true))=>return Fetched::Text("An unexpected error occurred.".into(),true),Err((why,false))=>out.push((id,name,Err(sync::scrub(&why,&credentials))))}
            }
            Fetched::Orders(user,local,ids,out)
        },
    }
}
pub fn finish(c:&Connection,fetch:Fetched)->Result<Value,WebError>{
    let user=match &fetch{Fetched::Balances(u,..)|Fetched::Orders(u,..)=>Some(*u),Fetched::Text(..)=>None};
    if let Some(user)=user{if !read_limits::check(c,user)?{return Ok(tool_text(read_limits::REFUSAL,false))}}
    let text=match fetch{
        Fetched::Text(text,is_error)=>return Ok(tool_text(read_limits::text(&text),is_error)),
        Fetched::Balances(user,exchange,name,cash,held)=>{
            let catalog=sync::balances::catalog_for(c,user,exchange).map_err(|_|error())?;
            let mut lines=vec![];
            for (id,qty) in match sync::balances::complete_balances(&catalog,cash,held){Ok(rows)=>rows,Err(why)=>return Ok(tool_text(why,false))}{
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
                    Ok(value)=>{
                        let rows=value.as_array().ok_or_else(error)?;
                        if !read_limits::count(rows.len(),read_limits::VENUE_ORDERS){return Ok(tool_text(read_limits::REFUSAL,false))}
                        for raw in rows{
                        let id=raw["id"].as_str().unwrap_or("");if ids.contains(id){continue}
                        let parsed=crate::venue::alpaca::parse_read_order(id,raw).map_err(|_|error())?;
                        let pair:Option<(String,String)>=c.query_row("SELECT base,quote FROM tickers WHERE exchange_id=?1 AND ticker=?2 LIMIT 1",rusqlite::params![exchange,parsed.pair],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                        let pair=pair.map(|(a,b)|format!("{a}/{b}")).unwrap_or_else(||id.into());
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
    };Ok(tool_text(read_limits::text(&text),false))
}
fn local_orders(c:&Connection,user:i64,exchange:Option<i64>)->Result<(Vec<String>,HashSet<String>),WebError>{
    let n:usize=c.query_row("SELECT count(*) FROM (SELECT t.id FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND t.status=0 AND t.external_status=1 AND (?2 IS NULL OR t.exchange_id=?2) LIMIT 101)",rusqlite::params![user,exchange],|r|r.get(0))?;
    if !read_limits::count(n,read_limits::LOCAL_ORDERS){return Ok((vec![read_limits::REFUSAL.into()],HashSet::new()))}
    let zone:String=c.query_row("SELECT time_zone FROM users WHERE id=?1",[user],|r|r.get(0))?;
    let mut q=c.prepare("SELECT t.id,t.created_at,t.side,t.amount,t.base,t.quote,t.price,t.order_type,e.name,t.external_id FROM transactions t JOIN bots b ON b.id=t.bot_id JOIN exchanges e ON e.id=t.exchange_id WHERE b.user_id=?1 AND t.status=0 AND t.external_status=1 AND (?2 IS NULL OR t.exchange_id=?2) ORDER BY t.created_at DESC LIMIT 100")?;
    let rows=q.query_map(rusqlite::params![user,exchange],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,tools::number(r,3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,tools::number(r,6)?,r.get::<_,Option<i64>>(7)?,r.get::<_,String>(8)?,r.get::<_,Option<String>>(9)?)))?;
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
