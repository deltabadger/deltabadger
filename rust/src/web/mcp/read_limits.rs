//! MCP read admission checks. SQL reads only lengths before loading stored strings.
use crate::web::WebError;
use rusqlite::Connection;
pub const CATALOG:usize=20_000;
pub const BOTS:usize=100;
pub const ORDERS:usize=10_000;
pub const LOCAL_ORDERS:usize=100;
pub const VENUE_ORDERS:usize=50;
pub const STRING:usize=16_384;
pub const STORED_BYTES:usize=4*1024*1024;
pub const TEXT:usize=65_536;
pub const REFUSAL:&str="Read unavailable: request data exceeds read limits";
pub fn text(s:&str)->&str {if s.len()>TEXT{"Read unavailable: response exceeds 65536 bytes"}else{s}}
pub fn count(n:usize,limit:usize)->bool {n<=limit}
/// All identifiers below come from static callers or SQLite schema, never request data.
pub fn table(c:&Connection,table:&str,filter:&str,cap:usize,user:i64,total:&mut usize)->Result<bool,WebError>{
    let cols=c.prepare(&format!("PRAGMA table_info({table})"))?.query_map([],|r|r.get::<_,String>(1))?.collect::<Result<Vec<_>,_>>()?;
    let lengths=cols.iter().map(|name|format!("COALESCE(length(CAST(\"{}\" AS BLOB)),0)",name.replace('"',"\"\""))).collect::<Vec<_>>();
    let sql=format!("SELECT {} FROM {table} WHERE {filter} LIMIT {}",lengths.join(","),cap+1);
    let mut q=c.prepare(&sql)?;
    let mut rows=q.query([user])?;
    let mut n=0;
    while let Some(row)=rows.next()? {
        n+=1; if !count(n,cap){return Ok(false)}
        for i in 0..cols.len(){
            let size=row.get::<_,usize>(i)?;
            *total=total.saturating_add(size);
            if size>STRING||*total>STORED_BYTES{return Ok(false)}
        }
    }
    Ok(true)
}
pub fn check(c:&Connection,user:i64)->Result<bool,WebError>{
    let mut bytes=0;
    for (table_name,filter,cap) in [
        ("indices","?1 IS NOT NULL",CATALOG),("assets","?1 IS NOT NULL",CATALOG),("tickers","?1 IS NOT NULL",CATALOG),("exchange_assets","?1 IS NOT NULL",CATALOG),
        ("exchanges","?1 IS NOT NULL",100),("users","id=?1",1),("api_keys","user_id=?1",100),
        ("bots","user_id=?1",BOTS),("transactions","bot_id IN (SELECT id FROM bots WHERE user_id=?1)",ORDERS),
        ("bot_index_assets","bot_id IN (SELECT id FROM bots WHERE user_id=?1)",ORDERS),
        ("wash_sale_locks","user_id=?1",ORDERS),("account_transactions","user_id=?1",ORDERS),
    ]{if !table(c,table_name,filter,cap,user,&mut bytes)?{return Ok(false)}}
    Ok(true)
}
/// Even an empty-history bot consumes the enclosing figures budget before settings/member work.
pub fn charge_bot()->Result<(),WebError>{crate::figures::budget::charge(1,0).map_err(|_|WebError::Config("Read unavailable: figures budget exceeded".into()))}
