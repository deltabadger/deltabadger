//! The only reader of stored fill columns. Accounting consumes quantity and value together.
use super::{db::Order,dec::Dec,FiguresError};
#[derive(Clone,Debug)]
pub struct Raw {price:Option<Dec>,amount:Option<Dec>,amount_exec:Option<Dec>,quote_amount_exec:Option<Dec>}
impl Raw {
    pub fn new(price:Option<Dec>,amount:Option<Dec>,amount_exec:Option<Dec>,quote_amount_exec:Option<Dec>)->Self{
        Self{price,amount,amount_exec,quote_amount_exec}
    }
}
#[derive(Clone,Debug,PartialEq)]
pub struct Fill {pub quantity:Dec,pub value:Dec}
impl Fill {
    pub fn unit_price(&self)->Result<Dec,FiguresError>{Ok(self.value.div(&self.quantity)?)}
}
/// A closed legacy row falls back to requested quantity only when execution quantity is NULL.
/// Nonpositive/absent execution moves nothing. Positive execution must carry a known value.
/// Reported positive value wins; otherwise multiply positive unit price by effective quantity exactly.
pub fn parse(order:&Order)->Result<Option<Fill>,FiguresError>{
    parse_raw(&order.raw, order.closed)
}
fn parse_raw(raw: &Raw, closed: bool) -> Result<Option<Fill>, FiguresError> {
    let quantity=raw.amount_exec.as_ref().or(if closed{raw.amount.as_ref()}else{None});
    let Some(quantity)=quantity.filter(|q|q.is_positive())else{return Ok(None)};
    let value=if let Some(value)=raw.quote_amount_exec.as_ref().filter(|v|v.is_positive()){
        value.clone()
    }else if let Some(price)=raw.price.as_ref().filter(|p|p.is_positive()){
        (price*quantity)?
    }else{return Err(FiguresError::NotComputed("executed fill value unavailable".into()))};
    Ok(Some(Fill{quantity:quantity.clone(),value}))
}

use rusqlite::{params,Connection};
use crate::enums::{TxExternalStatus,TxSide,TxStatus};
use super::budget;
pub(super) fn orders(c: &Connection, bot_id: i64) -> Result<Vec<Order>, FiguresError> {
    let mut statement = c.prepare(
        "SELECT id, created_at, exchange_id, price, amount, amount_exec, quote_amount_exec, base, base_asset_id, side, external_status, transaction_type \
         FROM transactions WHERE bot_id = ?1 AND status = ?2 ORDER BY created_at ASC, id ASC")?;
    let mut rows = statement.query(params![bot_id, TxStatus::Submitted as i64])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        budget::charge(1, 0)?;
        let (side, status): (Option<i64>, Option<i64>) = (r.get(9)?, r.get(10)?);
        out.push(Order {
            id: r.get(0)?, at: super::db::instant(&r.get::<_, String>(1)?)?, exchange_id: r.get(2)?,
            raw: Raw::new(super::db::decimal(r, 3)?, super::db::decimal(r, 4)?, super::db::decimal(r, 5)?, super::db::decimal(r, 6)?),
            base: r.get(7)?, asset_id: r.get(8)?, sell: side == Some(TxSide::Sell as i64), buy: side == Some(TxSide::Buy as i64),
            closed: status == Some(TxExternalStatus::Closed as i64),
            kind: r.get::<_, Option<String>>(11)?.unwrap_or_default(), // NULL kind has no special accounting role; DB errors propagate.
        });
    }
    Ok(out)
}

/// An engine commitment: normalized settled value or the quote reserved by a waiting buy.
/// SQL numeric kind survives only for Rails' existing amount-limit compatibility calculation.
pub struct Commitment { pub status: i64, pub value: crate::ruby::BigDec, pub cap_value: crate::ruby::Num }

pub fn commitments(c: &Connection, bot_id: i64, since: &str) -> Result<Vec<Commitment>, FiguresError> {
    budget::within(|| commitments_bounded(c, bot_id, since))
}
fn commitments_bounded(c: &Connection, bot_id: i64, since: &str) -> Result<Vec<Commitment>, FiguresError> {
    use crate::ruby::{BigDec, Num};
    let mut s = c.prepare("SELECT external_status, quote_amount, price, amount, amount_exec, quote_amount_exec FROM transactions WHERE bot_id=?1 AND status=0 AND side=0 AND transaction_type='REGULAR' AND external_status IN (0,1,2,3,4) AND created_at>=?2 ORDER BY id")?;
    let mut rows = s.query(params![bot_id,since])?;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        budget::charge(1,0)?;
        let status: i64 = r.get(0)?;
        let raw = Raw::new(super::db::decimal(r,2)?,super::db::decimal(r,3)?,super::db::decimal(r,4)?,super::db::decimal(r,5)?);
        let fill = parse_raw(&raw,status==2)?;
        let value = if status==0 || status==1 {
            match (super::db::decimal(r,1)?,&raw.amount,&raw.price) {
                (Some(q),_,_)=>q,
                (None,Some(a),Some(p))=>(a*p)?,
                _=>return Err(FiguresError::Data("waiting order with neither quote_amount nor amount×price".into())),
            }
        } else {
            // A known credit with no quantity is still money already spent. Eligibility rejects
            // that unreadable holding; a standalone carry read must never re-owe the credit.
            fill.map_or_else(||raw.quote_amount_exec.clone().unwrap_or_else(Dec::zero),|f|f.value)
        };
        let value = BigDec::parse(&value.to_s_f()).map_err(|_| FiguresError::Data("commitment outside engine decimal bounds".into()))?;
        let cap_value = if status==3 || status==4 {
            match r.get_ref(5)? {
                rusqlite::types::ValueRef::Integer(n) if value==BigDec::from_i64(n)=>Num::Int(n),
                rusqlite::types::ValueRef::Real(f) if raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive)=>Num::Float(f),
                rusqlite::types::ValueRef::Null if value.is_zero()=>Num::Int(0),
                _=>Num::Dec(value.clone()),
            }
        } else { Num::Dec(value.clone()) };
        out.push(Commitment{status,value,cap_value});
    }
    Ok(out)
}

/// Waiting buys claim only requested units still unfilled. Fill validation is shared with the walk.
pub fn reserved(c: &Connection,bot_id:i64)->Result<std::collections::HashMap<i64,crate::ruby::BigDec>,FiguresError>{
    use crate::ruby::BigDec;
    let mut s=c.prepare("SELECT base_asset_id,price,amount,amount_exec,quote_amount_exec FROM transactions WHERE bot_id=?1 AND status=0 AND external_status IN (0,1) AND side=0")?;
    let mut rows=s.query([bot_id])?;
    let mut out=std::collections::HashMap::new();
    while let Some(r)=rows.next()? {
        budget::charge(1,0)?;
        let raw=Raw::new(super::db::decimal(r,1)?,super::db::decimal(r,2)?,super::db::decimal(r,3)?,super::db::decimal(r,4)?);
        let fill=parse_raw(&raw,false)?;
        let requested=raw.amount.clone().unwrap_or_else(Dec::zero);
        let filled=fill.map_or_else(Dec::zero,|f|f.quantity);
        let remainder=(&requested-&filled)?;
        if !remainder.is_positive(){continue;}
        let asset=r.get::<_,Option<i64>>(0)?.ok_or_else(||FiguresError::Data("resting order without base_asset_id".into()))?;
        let remainder=BigDec::parse(&remainder.to_s_f()).map_err(|_|FiguresError::Data("reservation outside engine decimal bounds".into()))?;
        let held=out.remove(&asset).unwrap_or_else(BigDec::zero);
        out.insert(asset,held.checked_add(&remainder).map_err(|_|FiguresError::Data("reservation outside engine decimal bounds".into()))?);
    }
    Ok(out)
}

use crate::engine::EngineError;
use crate::ruby::{BigDec, from_sql};
pub(crate) struct StoredOrder {
    pub id: i64, pub side: Option<i64>, pub external_status: Option<i64>,
    pub price: Option<BigDec>, pub amount: Option<BigDec>, pub quote_amount: Option<BigDec>, pub amount_exec: Option<BigDec>, pub quote_amount_exec: Option<BigDec>,
    pub order_type: Option<i64>, pub base: Option<String>, pub quote: Option<String>, pub base_asset_id: Option<i64>, pub quote_asset_id: Option<i64>,
}

pub(crate) fn stored_order(c: &Connection, id: i64) -> Result<StoredOrder, EngineError> {
    let row = c.query_row(
        "SELECT id, side, external_status, price, amount, quote_amount, amount_exec, quote_amount_exec, \
         order_type, base, quote, base_asset_id, quote_asset_id FROM transactions WHERE id = ?1", [id],
        |r| {
            let dec = |i: usize| from_sql(r.get_ref(i)?).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")));
            Ok(StoredOrder { id: r.get(0)?, side: r.get(1)?, external_status: r.get(2)?,
                     price: dec(3)?, amount: dec(4)?, quote_amount: dec(5)?, amount_exec: dec(6)?, quote_amount_exec: dec(7)?,
                     order_type: r.get(8)?, base: r.get(9)?, quote: r.get(10)?, base_asset_id: r.get(11)?, quote_asset_id: r.get(12)? })
        })?;
    Ok(row)
}


/// Trading cannot size an unreadable holding, even when its spent quote is known.
pub fn for_engine(order: &Order) -> Result<Option<Fill>, FiguresError> {
    let fill = parse(order)?;
    if fill.is_none() && order.raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive) {
        return Err(FiguresError::Data("executed fill quantity unavailable".into()));
    }
    Ok(fill)
}
