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
    let raw=&order.raw;
    let quantity=raw.amount_exec.as_ref().or(if order.closed{raw.amount.as_ref()}else{None});
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
