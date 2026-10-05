//! The legacy DcaSingleAsset/Signal ledger, sharing decimal, row, split and market readers.
use super::{at::At,budget,db::{self,Bot,Kind,Subject},num::Num,walk::Metrics,splits::{self,Holding,Event},market::MarketData,live,FiguresError};
use rusqlite::Connection;
use serde_json::Value;
pub struct Pair {pub subject:Subject,pub metrics:Metrics,pub amount:Num,pub average:Option<Num>,cash:Num,restated:Option<At>,unresolved:bool}
fn bad()->FiguresError{FiguresError::Data("Invalid pair settings".into())}
pub fn load(c:&Connection,id:i64)->Result<Subject,FiguresError>{
    let (user,exchange,settings):(i64,i64,String)=c.query_row("SELECT user_id,exchange_id,settings FROM bots WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let v:Value=serde_json::from_str(&settings).map_err(|_|bad())?;
    let quote=v["quote_asset_id"].as_i64();let base=v["base_asset_id"].as_i64();
    let kind:String=c.query_row("SELECT type FROM exchanges WHERE id=?1",[exchange],|r|r.get(0))?;
    // Only the catalogue shape is shared with a basket; this module owns the distinct pair walk.
    let bot=Bot{id,user_id:user,exchange_id:Some(exchange),kind:Kind::Basket,exchange_type:Some(kind),quote_asset_id:quote,base_asset_ids:base.into_iter().collect()};
    let tickers=db::pair_ticker(c,&bot)?;
    Ok(Subject{orders:db::orders(c,id)?,tickers,quote:db::asset_symbol(c,quote)?,bot})
}
fn pnl(m:&mut Metrics)->Result<(),FiguresError>{
    m.pnl=Some(if m.total_quote_amount_invested.is_zero(){Num::Float(0.0)}else{Num::Float(m.total_amount_value_in_quote.sub(&m.total_quote_amount_invested)?.to_f()).div(&m.total_quote_amount_invested)?});Ok(())
}
impl Pair {
    fn split(&mut self,e:&Event,buys:&mut [(Num,Num)],last:&mut Option<Num>)->Result<(),FiguresError>{
        let held=!self.amount.is_zero();let factor=Num::Dec(e.factor.clone());
        self.amount=self.amount.mul(&factor)?;
        for (price,amount) in buys{*price=price.div(&factor)?;*amount=amount.mul(&factor)?;}
        if let Some(price)=last{*price=price.div(&factor)?;}
        if held{
            self.restated=Some(e.at);
            self.metrics.total_amount_value_in_quote=self.cash.add(&self.amount.mul(last.as_ref().unwrap_or(&Num::Int(0)))?)?;
            self.metrics.chart.labels.push(e.at);
        }Ok(())
    }
}
pub fn metrics(c:&Connection,id:i64,now:At)->Result<Pair,FiguresError>{
    budget::within(||{
        let s=load(c,id)?;let orders=s.orders.clone();
        let mut fills:Vec<Option<super::fill::Fill>>=Vec::with_capacity(orders.len());
        for order in &orders{budget::charge(1,0)?;fills.push(super::fill::parse(order)?);}
        let mut p=Pair{subject:s,metrics:Metrics::empty(),amount:Num::Int(0),average:None,cash:Num::Int(0),restated:None,unresolved:false};
        if orders.is_empty(){return Ok(p)}
        let holdings=[Holding{key:"pair".into(),asset_id:p.subject.bot.base_asset_ids.first().copied(),strings:orders.iter().filter_map(|o|o.base.clone()).collect()}];
        let events=splits::events(c,p.subject.bot.user_id,&orders,&holdings,now)?;
        p.unresolved=splits::unresolved(c,p.subject.bot.user_id,&orders,&holdings,now)?;
        let mut events=events.iter().peekable();let mut buys=vec![];let mut last=None;
        for (o,fill) in orders.iter().zip(fills){
            while events.peek().is_some_and(|e|e.at<=o.at){if let Some(e)=events.next(){p.split(e,&mut buys,&mut last)?;}}
            budget::charge(1,0)?;
            let Some(fill)=fill else{continue};
            let price=Num::Dec(fill.unit_price()?);
            let amount=Num::Dec(fill.quantity);
            let cost=Num::Dec(fill.value);
            if o.sell{
                let excess=Num::max2(amount.sub(&p.amount)?,Num::Int(0))?;
                p.cash=p.cash.add(&cost)?;p.amount=Num::max2(p.amount.sub(&amount)?,Num::Int(0))?;
                if excess.is_positive(){p.metrics.total_quote_amount_invested=p.metrics.total_quote_amount_invested.add(&cost.mul(&excess)?.div(&amount)?)?;}
            }else{
                p.metrics.total_quote_amount_invested=p.metrics.total_quote_amount_invested.add(&cost)?;
                p.amount=p.amount.add(&amount)?;buys.push((price.clone(),amount));
            }
            p.metrics.total_amount_value_in_quote=p.cash.add(&p.amount.mul(&price)?)?;last=Some(price);
            p.metrics.chart.labels.push(o.at);
        }
        for e in events{p.split(e,&mut buys,&mut last)?;}
        if !buys.is_empty(){
            let (mut cost,mut amount)=(Num::Int(0),Num::Int(0));
            for (price,qty) in buys{cost=cost.add(&price.mul(&qty)?)?;amount=amount.add(&qty)?;}
            p.average=Some(cost.div(&Num::Float(amount.to_f()))?);
        }
        pnl(&mut p.metrics)?;Ok(p)
    })
}
pub fn marked(c:&Connection,id:i64,now:At,market:&dyn MarketData)->Result<Pair,FiguresError>{
    let mut p=metrics(c,id,now)?;
    if p.metrics.chart.labels.is_empty(){return Ok(p)}
    let Some(ticker)=p.subject.tickers.first()else{p.metrics.prices_stale=p.amount.is_positive();return Ok(p)};
    if p.unresolved||p.restated.is_some_and(|at|now.minus(at)<splits::QUARANTINE_SECONDS as f64){p.metrics.prices_stale=true;return Ok(p)}
    match market.prices(&live::venue(&p.subject)?,std::slice::from_ref(&ticker.ticker)){
        Ok(prices)=>if let Some((_,Ok(price)))=prices.iter().find(|(key,_)|key==&ticker.ticker){
            p.metrics.total_amount_value_in_quote=p.cash.add(&p.amount.mul(&Num::Dec(price.clone()))?)?;pnl(&mut p.metrics)?;
        }else{p.metrics.prices_stale=true;},
        Err(_)=>p.metrics.prices_stale=true, // The caller prints unavailable, never a stale live total.
    }Ok(p)
}
