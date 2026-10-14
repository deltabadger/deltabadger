//! Cold-ledger page figures. No history-dependent amount is invented while the walk warms.
use crate::figures::{dec::Dec,FiguresError};
use rusqlite::Connection;
use chrono::{DateTime,Utc};
#[derive(Clone,Debug)]
pub struct Holding {pub asset:i64,pub symbol:String,pub name:String,pub category:Option<String>,pub color:Option<String>,pub image:Option<String>,pub value:Dec}
#[derive(Clone,Debug)]
pub struct Cold {pub holdings:Vec<Holding>,pub value:Option<Dec>,pub synced:Option<DateTime<Utc>>,pub stale:bool,pub unpriced:Vec<String>,pub only_cash:bool,pub reason:Option<&'static str>}
fn time(raw:Option<String>)->Result<Option<DateTime<Utc>>,FiguresError>{raw.map(|s|crate::codec::parse_time(&s).map_err(|_|FiguresError::Data("unreadable balance timestamp".into()))).transpose()}
/// Rails' waiting_for_ledger groups by asset identity, not symbol. Predicates stay in SQLite.
pub fn cold(c:&Connection,owner:i64,exchange:Option<i64>,show_cash:bool)->Result<Cold,FiguresError>{
    // One snapshot: the balances and the stamp saying which key produced them.
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=cold(&tx,owner,exchange,show_cash)?;tx.commit()?;return Ok(out)}
    let mut s=c.prepare("SELECT a.id,a.symbol,a.name,a.category,a.color,a.image_url,b.usd_value,b.priced_at,b.synced_at FROM account_balances b JOIN assets a ON a.id=b.asset_id WHERE b.user_id=?1 AND (?2 IS NULL OR b.exchange_id=?2) AND b.free+b.locked>0 ORDER BY b.id")?;
    let mut q=s.query(rusqlite::params![owner,exchange])?;
    let mut holdings:Vec<Holding>=vec![];let mut synced=None;let mut oldest=None;let mut unpriced=vec![];let mut unpriced_assets=vec![];let mut missing_time=false;
    while let Some(r)=q.next()? {
        crate::figures::budget::charge(1,0)?;
        let id=r.get(0)?;let symbol=r.get::<_,Option<String>>(1)?.unwrap_or_default();
        let value=super::scaled(r.get_ref(6)?,8)?;
        let price_time=time(r.get(7)?)?;let balance_time=time(r.get(8)?)?;
        synced=synced.max(balance_time);
        if value.as_ref().is_some_and(Dec::is_positive){
            if let Some(at)=price_time {oldest=Some(oldest.map_or(at,|old:DateTime<Utc>|old.min(at)));}else{missing_time=true;}
        }else if !unpriced_assets.contains(&id){unpriced_assets.push(id);unpriced.push(symbol.clone());}
        // A missing amount remains unavailable. The placeholder is never summed or displayed.
        let Some(value)=value else {continue};
        if let Some(h)=holdings.iter_mut().find(|h|h.asset==id){h.value=(&h.value+&value)?;}else{
            holdings.push(Holding{asset:id,symbol,name:r.get::<_,Option<String>>(2)?.unwrap_or_default(),category:r.get(3)?,color:r.get(4)?,image:r.get(5)?,value});
        }
    }
    let mut s=c.prepare("SELECT balances_synced_at FROM api_keys WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2)")?;
    for mark in s.query_map(rusqlite::params![owner,exchange],|r|r.get::<_,Option<String>>(0))?{synced=synced.max(time(mark?)?);}
    let stale=oldest.zip(synced).is_some_and(|(p,s)|s.signed_duration_since(p)>chrono::Duration::seconds(300));
    // Balances not produced by a completed sync of the current key are what the navbar already withholds.
    let unsynced=crate::sync::cache::stale(c,owner,exchange).map_err(|_|FiguresError::Data("balance cache provenance unavailable".into()))?;
    let reason=if unsynced{Some("Tracker figures unavailable: balances not yet synced with the current key")}else if stale{Some("Tracker figures unavailable: stale prices")}else if missing_time{Some("Tracker figures unavailable: price timestamp missing")}else if !unpriced.is_empty(){Some("Tracker figures unavailable: unpriced holdings")}else{None};
    holdings.sort_by(|a,b|b.value.cmp(&a.value));
    let had_holdings=!holdings.is_empty();
    if !show_cash{holdings.retain(|h|!super::super::cash(&h.symbol));}
    let only_cash=had_holdings && holdings.is_empty();
    let value=if reason.is_some(){None}else{Some(holdings.iter().try_fold(Dec::zero(),|sum,h|&sum+&h.value)?)};
    Ok(Cold{holdings,value,synced,stale,unpriced,only_cash,reason})
}

/// The page's figures once the ledger is warm (`Tracker::Figures.for` with the cached scope), restated without the cash
/// when the switch hides it (`Result#without_cash`), and whether that left nothing but cash (`@portfolio_all_cash`).
pub struct Warm {pub figures:super::Figures,pub all_cash:bool,pub received:Dec}
pub fn warm(c:&Connection,owner:i64,exchange:Option<i64>,show_cash:bool,ledger:&crate::tracker::walk::Summary,fx:&crate::tracker::fx::FxTable,today:chrono::NaiveDate)->Result<Warm,FiguresError>{
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=warm(&tx,owner,exchange,show_cash,ledger,fx,today)?;tx.commit()?;return Ok(out)}
    let balances=super::balances(c,owner,exchange)?;
    let pending=super::pending(c,owner,exchange)?;
    let figures=super::compute_on(c,owner,ledger,&balances,&pending,fx,today)?;
    let received=ledger.received.clone();
    if show_cash {return Ok(Warm{figures,all_cash:false,received})}
    let invested=figures.without_cash_result()?;
    Ok(Warm{all_cash:!figures.holdings.is_empty() && invested.holdings.is_empty(),figures:invested,received})
}
