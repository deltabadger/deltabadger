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
/// Valid zero/absent execution moves nothing. Malformed fields always refuse before skipping.
/// Reported positive value wins; otherwise multiply positive unit price by effective quantity exactly.
pub fn parse(order:&Order)->Result<Option<Fill>,FiguresError>{
    parse_raw(&order.raw, order.closed)
}
fn parse_raw(raw: &Raw, closed: bool) -> Result<Option<Fill>, FiguresError> {
    validate_raw(raw)?;
    let quantity=raw.amount_exec.as_ref().or(if closed{raw.amount.as_ref()}else{None});
    let Some(quantity)=quantity.filter(|q|q.is_positive())else{return Ok(None)};
    let value=if let Some(value)=raw.quote_amount_exec.as_ref().filter(|v|v.is_positive()){
        value.clone()
    }else if let Some(price)=raw.price.as_ref().filter(|p|p.is_positive()){
        (price*quantity)?
    }else{return Err(FiguresError::NotComputed("executed fill value unavailable".into()))};
    bound(quantity)?; bound(&value)?;
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
pub use crate::engine::accounting::Commitment;
use crate::engine::accounting::{CapKind,SoldCommitment};

pub fn commitments(c: &Connection, bot_id: i64, since: &str) -> Result<Vec<Commitment>, FiguresError> {
    budget::within(|| commitments_bounded(c, bot_id, since, None))
}
/// The web action's existing work limit also bounds its normalized snapshot.
pub fn action_commitments(c: &Connection, bot_id: i64, since: &str) -> Result<Vec<Commitment>, FiguresError> {
    budget::within(|| commitments_bounded(c, bot_id, since, Some(100_000)))
}
fn commitments_bounded(c: &Connection, bot_id: i64, since: &str, limit: Option<usize>) -> Result<Vec<Commitment>, FiguresError> {
    use crate::ruby::BigDec;
    let mut s = c.prepare("SELECT external_status, quote_amount, price, amount, amount_exec, quote_amount_exec, created_at FROM transactions WHERE bot_id=?1 AND status=0 AND side=0 AND transaction_type='REGULAR' AND external_status IN (0,1,2,3,4) ORDER BY id")?;
    let since=crate::codec::parse_time(since).map_err(|e|FiguresError::Data(format!("{e:?}")))?;
    let mut rows = s.query([bot_id])?;
    let mut scanned=0;
    let mut out = vec![];
    while let Some(r) = rows.next()? {
        budget::charge(1,0)?;
        if limit.is_some_and(|limit| scanned >= limit) {
            return Err(FiguresError::Data("bot action history exceeds the 100000-row work budget".into()));
        }
        scanned+=1;
        let at=crate::codec::parse_time(&r.get::<_,String>(6)?).map_err(|e|FiguresError::Data(format!("{e:?}")))?;
        if at<since {continue;}
        let status: i64 = r.get(0)?;
        let raw = Raw::new(super::db::decimal(r,2)?,super::db::decimal(r,3)?,super::db::decimal(r,4)?,super::db::decimal(r,5)?);
        let fill = parse_raw(&raw,status==2)?;
        require_closed_quantity(&raw, status==2)?;
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
        bound(&value)?;
        let value = BigDec::parse(&value.to_s_f()).map_err(|_| FiguresError::Data("commitment outside engine decimal bounds".into()))?;
        let cap_kind = if status==3 || status==4 {
            match r.get_ref(5)? {
                rusqlite::types::ValueRef::Integer(n) if value==BigDec::from_i64(n)=>CapKind::Integer(n),
                rusqlite::types::ValueRef::Real(f) if raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive)=>CapKind::RealBits(f.to_bits()),
                rusqlite::types::ValueRef::Null if value.is_zero()=>CapKind::Integer(0),
                _=>CapKind::Decimal,
            }
        } else { CapKind::Decimal };
        let zero_without_report = matches!(status,3|4) && matches!(r.get_ref(5)?,rusqlite::types::ValueRef::Null) && value.is_zero();
        out.push(Commitment::normalized(status,value,cap_kind,zero_without_report));
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
        bound(&requested)?;
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

/// Typed numeric-read failure retained across stored snapshots and their web consumers.
#[derive(Debug)]
pub(crate) struct UnreadableFill;
impl std::fmt::Display for UnreadableFill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("unreadable stored fill number") }
}
impl std::error::Error for UnreadableFill {}

pub(crate) fn stored_order(c: &Connection, id: i64) -> Result<StoredOrder, EngineError> {
    let row = c.query_row(
        "SELECT id, side, external_status, price, amount, quote_amount, amount_exec, quote_amount_exec, \
         order_type, base, quote, base_asset_id, quote_asset_id FROM transactions WHERE id = ?1", [id],
        |r| {
            let dec = |i: usize| {
                let value = r.get_ref(i)?;
                from_sql(value).map_err(|_| rusqlite::Error::FromSqlConversionFailure(i,value.data_type(),Box::new(UnreadableFill)))
            };
            Ok(StoredOrder { id: r.get(0)?, side: r.get(1)?, external_status: r.get(2)?,
                     price: dec(3)?, amount: dec(4)?, quote_amount: dec(5)?, amount_exec: dec(6)?, quote_amount_exec: dec(7)?,
                     order_type: r.get(8)?, base: r.get(9)?, quote: r.get(10)?, base_asset_id: r.get(11)?, quote_asset_id: r.get(12)? })
        })?;
    Ok(row)
}


/// Trading cannot size an unreadable holding, even when its spent quote is known.
pub fn for_engine(order: &Order) -> Result<Option<Fill>, FiguresError> {
    let fill = parse(order)?;
    require_closed_quantity(&order.raw, order.closed)?;
    if fill.is_none() && order.raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive) {
        return Err(FiguresError::Data("executed fill quantity unavailable".into()));
    }
    Ok(fill)
}

// RULING-B2A-R1 item 1: a NULL closed quantity is unknown, not a zero fill.
fn require_closed_quantity(raw: &Raw, closed: bool) -> Result<(), FiguresError> {
    if closed && raw.amount_exec.is_none() && raw.amount.is_none() && raw.quote_amount_exec.is_none() {
        return Err(FiguresError::Data("closed fill quantity unavailable".into()));
    }
    Ok(())
}


/// Display/polling snapshots retain the venue's requested/reported fields. They are not
/// accounting credits: money consumers use commitments/for_engine instead. Read lazily
/// by ID so an unshown pagination sentinel cannot fail the current page.
pub(crate) fn display_amounts(c: &Connection, id: i64) -> Result<[Option<BigDec>; 5], EngineError> {
    let row = stored_order(c, id)?;
    Ok([row.price, row.amount, row.quote_amount, row.amount_exec, row.quote_amount_exec])
}

/// A quantity-only consumer (sell cap or split notification) needs no invented fill price.
/// Share the normalizer's malformed-field validation and closed quantity fallback.
fn quantity(raw: &Raw, closed: bool) -> Result<Dec, FiguresError> {
    validate_raw(raw)?;
    require_closed_quantity(raw, closed)?;
    let quantity = raw.amount_exec.as_ref().or(if closed { raw.amount.as_ref() } else { None }).cloned().unwrap_or_else(Dec::zero);
    if quantity.is_zero() && raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive) {
        return Err(FiguresError::Data("executed fill quantity unavailable".into()));
    }
    Ok(quantity)
}
pub fn sold_commitments(c: &Connection, bot_id: i64, since: Option<&str>) -> Result<Vec<SoldCommitment>, FiguresError> {
    budget::within(|| {
        let mut s = c.prepare("SELECT external_status, price, amount, amount_exec, quote_amount_exec, created_at FROM transactions WHERE bot_id=?1 AND side=1 AND status=0 AND transaction_type='REGULAR' AND external_status IN (0,1,2,3,4) LIMIT 100001")?;
        let since=since.map(crate::codec::parse_time).transpose().map_err(|e|FiguresError::Data(format!("{e:?}")))?;
        let mut rows = s.query([bot_id])?;
        let mut out = Vec::new();
        let mut work = 0;
        while let Some(row) = rows.next()? {
            work += 1;
            if work > 100_000 { return Err(FiguresError::Data("bot action history exceeds the 100000-row work budget".into())); }
            budget::charge(1,0)?;
            let at=crate::codec::parse_time(&row.get::<_,String>(5)?).map_err(|e|FiguresError::Data(format!("{e:?}")))?;
            if since.is_none_or(|since|at<since) {continue;}
            let status:i64 = row.get(0)?;
            let raw = Raw::new(super::db::decimal(row,1)?,super::db::decimal(row,2)?,super::db::decimal(row,3)?,super::db::decimal(row,4)?);
            let executed = quantity(&raw,status==2)?;
            let committed = if status==0 || status==1 { raw.amount.unwrap_or_else(Dec::zero) } else { executed };
            out.push(SoldCommitment::normalized(BigDec::parse(&committed.to_s_f()).map_err(|_|FiguresError::Data("sell commitment exceeds numeric bounds".into()))?));
        }
        Ok(out)
    })
}

/// Split notifications use the same validated effective quantities as the fill walk.
/// `naming` is the sync module's fixed SQL predicate, never request text.
pub(crate) type SplitQuantity = (i64, Option<i64>, BigDec, String);
pub(crate) fn split_quantities(c: &Connection, naming: &str, exchange: i64, user: i64, symbol: &str, at: &str) -> Result<Vec<SplitQuantity>, FiguresError> {
    budget::within(|| {
        let mut s = c.prepare(&format!("SELECT bot_id,side,external_status,price,amount,amount_exec,quote_amount_exec,created_at FROM transactions WHERE {naming} AND created_at < ?4"))?;
        let mut rows = s.query(params![exchange,user,symbol,at])?;
        let mut out = vec![];
        while let Some(row) = rows.next()? {
            budget::charge(1,0)?;
            let status:Option<i64> = row.get(2)?;
            let raw = Raw::new(super::db::decimal(row,3)?,super::db::decimal(row,4)?,super::db::decimal(row,5)?,super::db::decimal(row,6)?);
            let amount = quantity(&raw,status==Some(2))?;
            if amount.is_zero() { continue; }
            let amount = BigDec::parse(&amount.to_s_f()).map_err(|_|FiguresError::Data("split quantity exceeds numeric bounds".into()))?;
            out.push((row.get(0)?,row.get(1)?,amount,row.get(7)?));
        }
        Ok(out)
    })
}

fn bound(value: &Dec) -> Result<(), FiguresError> {
    crate::engine::accounting::bounded_fill_amount(value).map_err(|_|FiguresError::Data("accounting magnitude exceeds 2^53".into()))
}
fn validate_raw(raw: &Raw) -> Result<(), FiguresError> {
    // RULING-B2A-R3: validate all fields, including those a fallback would not use.
    // Non-finite/unparsable SQL values already fail the shared decimal reader.
    for value in [&raw.amount, &raw.amount_exec, &raw.quote_amount_exec, &raw.price].into_iter().flatten() {
        if value.is_negative() { return Err(FiguresError::NotComputed("negative fill field".into())); }
    }
    Ok(())
}


/// Exact stored bytes for synthetic parity snapshots only; never a money calculation.
/// Keeping this here prevents a generic SELECT * from bypassing the fill-reader boundary.
pub(crate) fn parity_rows(c: &Connection, table: &str, order: &str) -> rusqlite::Result<(Vec<String>, Vec<Vec<rusqlite::types::Value>>)> {
    if !["bots","transactions","bot_activity_logs","bot_index_assets","assets","tickers","exchange_assets","indices","app_configs","account_transactions","account_balances","api_keys","portfolio_snapshots","portfolio_venue_snapshots","wash_sale_locks","historical_prices"].contains(&table)
        || !["id","date","exchange_id, date"].contains(&order) { return Err(rusqlite::Error::InvalidQuery); }
    let mut statement = c.prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))?;
    let names:Vec<String> = statement.column_names().iter().map(|n|n.to_string()).collect();
    let mut rows = statement.query([])?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        out.push((0..names.len()).map(|i|row.get(i)).collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok((names,out))
}

/// R9: every row timestamp an action/read can use is checked before selecting a money window.
/// It reads no money and obeys the existing action history bound.
pub fn validate_row_times(c:&Connection, bot_id:i64) -> Result<(),FiguresError> {
    let mut statement=c.prepare("SELECT created_at,updated_at FROM transactions WHERE bot_id=?1 LIMIT 100001")?;
    let mut rows=statement.query([bot_id])?;
    let mut count=0;
    while let Some(row)=rows.next()? {
        count+=1;
        if count>100000 {return Err(FiguresError::Data("bot action history exceeds the 100000-row work budget".into()));}
        for index in [0,1] {
            let text:Option<String>=row.get(index)?;
            text.as_deref().map(crate::codec::parse_time).transpose().map_err(|e|FiguresError::Data(format!("{e:?}")))?;
        }
    }
    Ok(())
}
