//! The populated first-sync state. Every later account state stays explicitly deferred.
use crate::web::WebError;
use rusqlite::Connection;

/// A complete first-sync page only: no history can be mislabeled as an empty record.
/// Keep the owner and the absence predicates in SQL, including zero balances.
pub fn supported(c:&Connection,owner:i64)->Result<bool,WebError>{
    Ok(c.query_row("SELECT
      EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM api_keys k LEFT JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND (e.type IS NULL OR e.type!='Exchanges::Alpaca' OR COALESCE(k.status,-1)!=1 OR COALESCE(k.key_type,-1) NOT IN (0,2) OR k.last_synced_at IS NOT NULL OR k.balances_synced_at IS NOT NULL OR COALESCE(k.last_sync_error,'')!=''))
      AND NOT EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM portfolio_venue_snapshots WHERE user_id=?1)",[owner],|r|r.get(0))?)
}


use super::{Ctx,layout};
use crate::web::{auth,shell::{self,Shell},i18n::{self,Arg},bots::{Segmented,SegmentedOption}};
use askama::Template;
use axum::{http::StatusCode,response::Response};
#[derive(Template)]
#[template(path="tracker/first_sync.html")]
struct Page<'a>{v:&'a Ctx,preferences:String,tax_report:String,sync:String,csrf:String,hidden:bool,show_cash:bool,scope:Option<String>,export_path:String,from:String,to:String,record_switch:String,hint:String,unavailable:String}

/// HTML date controls submit ISO days. Other Date.parse grammars remain an explicit later slice.
fn dates(ctx:&Ctx)->Result<Option<(String,String)>,WebError>{
    let mut dates=Vec::new();
    for key in ["from","to"] {
        let raw=ctx.params.query(key).unwrap_or("");
        if raw.trim().is_empty(){dates.push(if key=="to"{ctx.app.now().date_naive().to_string()}else{String::new()});continue;}
        if raw.len()!=10 || raw.as_bytes().get(4)!=Some(&b'-') || raw.as_bytes().get(7)!=Some(&b'-') || !raw.bytes().enumerate().all(|(i,b)|i==4||i==7||b.is_ascii_digit()){return Ok(None);}
        if raw < "1583-01-01" {return Ok(None);}
        chrono::NaiveDate::parse_from_str(raw,"%Y-%m-%d").map_err(|_|super::row::invalid())?;
        dates.push(raw.to_string());
    }
    let mut dates=dates.into_iter();
    Ok(dates.next().zip(dates.next()))
}

pub fn render(c:&Connection,ctx:&Ctx,owner:i64)->Result<Response,WebError>{
    let Some(user)=ctx.user()else{return Ok(auth::unauthenticated(ctx))};
    let Some((from,to))=dates(ctx)? else{return Ok(layout::refused(ctx,"tracker date syntax"))};
    let settings:Option<String>=c.query_row("SELECT tracker_settings FROM users WHERE id=?1",[owner],|r|r.get(0))?;
    let shape:serde_json::Value=match settings.as_deref(){Some(raw)=>serde_json::from_str(raw).map_err(|_|super::row::invalid())?,None=>serde_json::Value::Null};
    if !shape.is_null() && !shape.is_object(){return Err(super::row::invalid());}
    // Rails' show_cash? expects a JSON object; syntax and shape are checked before this helper.
    let show_cash=crate::web::bots::show_cash(settings.as_deref());
    let options=vec![SegmentedOption{value:"pos",label:i18n::text(ctx.locale,"tracker.positions",&[]),active:true,href:None},SegmentedOption{value:"tx",label:i18n::text(ctx.locale,"tracker.transactions",&[]),active:false,href:None}];
    let record_switch=format!("\n{}",Segmented{fluid:true,label:i18n::text(ctx.locale,"tracker.positions",&[]),key:Some("tracker-record"),options,links:false}.render()?.lines().filter(|line|!line.is_empty()).collect::<Vec<_>>().join("\n").replace("->","-&gt;").replace("</svg>\n","</svg>\n\n"));
    let value=i18n::text(ctx.locale,"bot.details.stats.portfolio_value",&[]);
    let invested=i18n::text(ctx.locale,"bot.details.stats.total_invested",&[]);
    let hint=i18n::t(ctx.locale,"tracker.tiles.total_pnl_hint",&[("value",Arg::Text(&value)),("invested",Arg::Text(&invested))]);
    // No balance sync has happened. An empty table cannot prove a zero account value.
    let reason=ctx.t("tracker.portfolio.never_synced");
    let unavailable=format!("<span class=\"no-value\" title=\"{reason}\">—</span>");
    let pairs=ctx.params.query.iter().filter(|(key,_)|matches!(key.as_str(),"exchange_id"|"from"|"to")).cloned().collect::<Vec<_>>();
    let query=crate::web::locale::switch_query(&pairs);
    let export_path=i18n::escape(&ctx.path(&format!("/tracker/export{}",if query.is_empty(){String::new()}else{format!("?{query}")})));
    let body=Page{v:ctx,preferences:format!("user_{owner}:preferences"),tax_report:format!("user_{owner}:tax_report"),sync:format!("user_{owner}:sync"),csrf:ctx.csrf_token(),hidden:user.hide_balances,show_cash,scope:ctx.params.query("exchange_id").filter(|s|!s.trim().is_empty()).map(i18n::escape),export_path,from,to,record_switch,hint,unavailable}.render()?;
    let page=layout::Page{status:StatusCode::OK,body,flash_now:vec![]};
    if ctx.turbo_frame.is_some(){return layout::frame(&ctx.csrf_token(),&page);}
    let shell=Shell::load(c,&ctx.app,user)?;
    shell::application_with_flash_extra(ctx,&ctx.csrf_token(),user,&shell,page,"  <div id=\"sync-warnings\" data-turbo-permanent>\n    \n  </div>\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db()->Connection {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges(id INTEGER,type TEXT); CREATE TABLE api_keys(user_id INTEGER,exchange_id INTEGER,status INTEGER,key_type INTEGER,last_synced_at TEXT,balances_synced_at TEXT,last_sync_error TEXT); CREATE TABLE account_transactions(user_id INTEGER); CREATE TABLE account_balances(user_id INTEGER); CREATE TABLE portfolio_snapshots(user_id INTEGER); CREATE TABLE portfolio_venue_snapshots(user_id INTEGER); INSERT INTO exchanges VALUES(1,'Exchanges::Alpaca'),(2,'Exchanges::Kraken'); INSERT INTO api_keys VALUES(7,1,1,0,NULL,NULL,NULL);").unwrap();
        c
    }
    #[test]
    fn first_sync_reads_only_the_owner_and_requires_an_unused_reading_alpaca_key() {
        let c=db();
        assert!(super::super::read::only(&c,|c|supported(c,7)).unwrap());
        assert!(!supported(&c,8).unwrap());
        for table in ["account_transactions","account_balances","portfolio_snapshots","portfolio_venue_snapshots"] {
            c.execute(&format!("INSERT INTO {table} VALUES(8)"),[]).unwrap();
            assert!(supported(&c,7).unwrap(),"foreign {table}");
            c.execute(&format!("INSERT INTO {table} VALUES(7)"),[]).unwrap();
            assert!(!supported(&c,7).unwrap(),"owned {table}");
            c.execute(&format!("DELETE FROM {table}"),[]).unwrap();
        }
        for change in ["status=0","status=2","status=NULL","key_type=1","key_type=NULL","last_synced_at='2026-01-01'","balances_synced_at='2026-01-01'","last_sync_error='failed'","exchange_id=2"] {
            let c=db();c.execute(&format!("UPDATE api_keys SET {change}"),[]).unwrap();
            assert!(!supported(&c,7).unwrap(),"{change}");
        }
        let c=db();c.execute("UPDATE api_keys SET key_type=2",[]).unwrap();assert!(supported(&c,7).unwrap());
        c.execute("INSERT INTO api_keys VALUES(8,2,2,1,'old','old','foreign')",[]).unwrap();assert!(supported(&c,7).unwrap());
        c.execute("INSERT INTO api_keys VALUES(7,2,1,0,NULL,NULL,NULL)",[]).unwrap();assert!(!supported(&c,7).unwrap());
    }
    #[test]
    fn first_sync_database_errors_are_propagated() {
        let c=db();c.execute("DROP TABLE account_balances",[]).unwrap();
        assert!(supported(&c,7).is_err());
    }
}
