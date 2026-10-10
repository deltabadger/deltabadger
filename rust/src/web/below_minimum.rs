//! Rails' first-own-tick modal. No request data, new route or session token enters a broadcast.
use super::{App,WebError,i18n::{self,Arg},turbo};
use askama::Template;
use rusqlite::{Connection,params,OptionalExtension};
use crate::engine::events::EngineEvent;
#[derive(Template)]
#[template(path="below_minimum.html")]
struct Modal {title:String,intro:String,buffer:String,note:String,understand:String,single:bool}

pub fn render(c:&Connection,bot_id:i64,ids:&[i64])->Result<(String,String),WebError>{
    let (owner,locale,exchange):(i64,Option<String>,String)=c.query_row("SELECT b.user_id,u.locale,e.name FROM bots b JOIN users u ON u.id=b.user_id JOIN exchanges e ON e.id=b.exchange_id WHERE b.id=?1",[bot_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let locale=locale.as_deref().unwrap_or("en");
    let count=i64::try_from(ids.len()).map_err(|_|WebError::Config("warning row count".into()))?;
    let intro=if let [id]=ids {
        let (base,quote):(String,String)=c.query_row("SELECT coalesce(a.symbol,t.base),coalesce(q.symbol,t.quote) FROM transactions t LEFT JOIN assets a ON a.id=t.base_asset_id LEFT JOIN assets q ON q.id=t.quote_asset_id WHERE t.bot_id=?1 AND t.id=?2",params![bot_id,id],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let minimums=c.query_row("SELECT k.minimum_base_size,k.minimum_quote_size FROM tickers k JOIN transactions t ON k.exchange_id=t.exchange_id AND k.base_asset_id=t.base_asset_id AND k.quote_asset_id=t.quote_asset_id WHERE t.bot_id=?1 AND t.id=?2 ORDER BY k.id LIMIT 1",params![bot_id,id],|r|Ok((crate::ruby::from_sql(r.get_ref(0)?).map_err(|_|rusqlite::Error::InvalidQuery)?,crate::ruby::from_sql(r.get_ref(1)?).map_err(|_|rusqlite::Error::InvalidQuery)?))).optional()?;
        let (min_base,min_quote)=minimums.unwrap_or((None,None));
        let min_base=min_base.map(|v|v.to_s_f()).unwrap_or_default();let min_quote=min_quote.map(|v|v.to_s_f()).unwrap_or_default();
        i18n::t(locale,"bot.warning.dca_single_asset.intro",&[("quote_symbol",Arg::Text(&quote)),("missed_symbol",Arg::Text(&base)),("missed_minimum_base_size",Arg::Text(&min_base)),("missed_minimum_quote_size",Arg::Text(&min_quote)),("exchange_name",Arg::Text(&exchange))])
    }else{
        i18n::t(locale,"bot.dca_index.warning.intro",&[("skipped_count",Arg::Count(count)),("exchange_name",Arg::Text(&exchange))])
    };
    let html=Modal{title:i18n::t(locale,"bot.warning.title",&[]),intro,buffer:i18n::t(locale,"bot.warning.buffer_explanation",&[("count",Arg::Count(count))]),note:i18n::t(locale,"bot.warning.general_note",&[]),understand:i18n::t(locale,"button.understand",&[]),single:count==1}.render()?;
    Ok((format!("user_{owner}:bot_updates"),turbo::stream("replace","modal",&html)))
}

pub async fn follow(app:App,mut events:tokio::sync::mpsc::UnboundedReceiver<EngineEvent>,mut stop:tokio::sync::watch::Receiver<bool>)->Result<(),String>{
    loop {
        tokio::select! {
            _=stop.wait_for(|stopped|*stopped)=>return Ok(()),
            event=events.recv()=>match event {
                Some(EngineEvent::BelowMinimum{bot_id,transaction_ids})=>{
                    let (stream,payload)=app.db(move|c|render(c,bot_id,&transaction_ids)).await.map_err(|_|"below-minimum modal could not be rendered".to_string())?;
                    app.hub.broadcast(&stream,&payload);
                }
                Some(_)=>{},
                None=>{ return Ok(()); }
            }
        }
    }
}
