//! Network work never owns the database lock. Only raw JSON and request descriptions cross threads.
use super::service::{Load,Work};
use crate::figures::{at::At,budget,db,page_market::{Cache,Reader}};
use crate::venue::http::{self,HttpRequest,HttpResponse,ReqwestTransport,Transport,TransportError};
use crate::web::{App,WebError};
use crate::crypto::Credentials;
use rusqlite::{Connection,OptionalExtension};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone,Default)]
pub enum Source { #[default] Live, Disabled, Script(Value) }
#[derive(Clone)]
/// Ready: the cache, its stamp, the revision it was prepared against, where the database stood when that revision was
/// read (`unchanged_since`), if on the connection that renders it, and until when (Unix seconds) its figures may be shown.
pub enum Snapshot { Cold, Failed(Option<&'static str>), Ready(Cache,At,u64,Option<Mark>,i64) }
pub(crate) struct Info { identity:String,revision:u64,mark:Mark,pub(crate) credentials:Credentials,pub(crate) origin:crate::engine::model::CredentialVersion }
/// Where a connection's view of the database stands: what other connections committed (`PRAGMA data_version`) and what
/// it wrote itself (total changes). Equal on that connection, nothing was written in between.
pub type Mark=(i64,u64);
fn mark(c:&Connection)->Result<Mark,rusqlite::Error> { Ok((c.query_row("PRAGMA data_version",[],|r|r.get(0))?,c.total_changes())) }
/// What this account's figures were filled for, as values: the rows Rails' metrics cache is recomputed for (a bot's
/// transactions, and the bot: its settings, status, venue, restatement, and the operation flags that hide its Sell and
/// Redeploy controls), the account's own rows the figures read, and the reference data they read: the account's
/// tickers (the composition's, wherever listed, and its venues' listings of the assets the bots' settings, compositions
/// and orders name or that the orders were placed under), what the split rows' names resolve to (`splits::lookups`), and
/// the assets all of these, the ledger and the balances name. Never a venue's whole listing: a sync rewrites thousands
/// of rows no figure of this account reads. Left out as well: `updated_at`, the rest of a bot's transient bookkeeping,
/// and what a sync rewrites that no figure reads (a market cap, an ATH). An index bot also reads its venue's other
/// tradable listings in its quote (`db::tickers`): what it asks the market for, and the coarsest quote precision among
/// them; only those of its holdings are here.
/// The scheduler records every job run (app_configs) and its syncs rewrite the reference data all the time, and a
/// whole-database revision never held figures from one page load to the next. A replay (`Service::latest`) is checked
/// against this alone, so anything a figure reads belongs here.
/// Also how many rows it read, for the tests that hold it to the account's own.
/// The resolved split names' asset ids as one compound-select term. A ` UNION VALUES` per id grows the statement with
/// every name: SQLite's 500-term limit where it is set, and where it is not, a parser that runs out of stack (5,000 terms
/// abort the process), on every page that loads figures.
fn ids_source(ids:impl Iterator<Item=i64>)->String {
    let ids:Vec<String>=ids.map(|id|id.to_string()).collect();
    if ids.is_empty(){String::new()}else{format!(" UNION SELECT CAST(value AS INTEGER) FROM json_each('[{}]')",ids.join(","))}
}

pub fn revision(c:&Connection,user:i64)->Result<(u64,usize),rusqlite::Error> {
    use std::hash::{Hash,Hasher};
    use rusqlite::types::ValueRef;
    use crate::figures::FiguresError;
    let mut hash=std::hash::DefaultHasher::new();
    let mut read=0;
    // A figure that cannot be computed is a value too: its reason, so long as it holds.
    fn computed<T>(result:Result<T,FiguresError>,hash:&mut impl Hasher)->Result<Option<T>,rusqlite::Error> {
        match result { Ok(v)=>Ok(Some(v)), Err(FiguresError::Sqlite(e))=>Err(e), Err(other)=>{format!("{other:?}").hash(hash);Ok(None)} }
    }
    // What each split row's name stands for on the venue it was reported on, as the walk of each bot's orders resolves
    // it. A name resolves alike for every bot that traded on its venue: one lookup over the venues of all their
    // submitted orders that name a base (splits::eligibility), read from those columns alone. Parsing the orders whole
    // here cost more than the rest of the fingerprint; an order that fails to parse is in the rows hashed below.
    let eligible=format!("SELECT DISTINCT exchange_id FROM transactions WHERE bot_id IN (SELECT id FROM bots WHERE user_id=?1 AND status!={}) \
        AND status={} AND (trim(base)<>'' OR base_asset_id IS NOT NULL) ORDER BY exchange_id",crate::enums::BotStatus::Deleted as i64,crate::enums::TxStatus::Submitted as i64);
    let venues_traded=c.prepare(&eligible)?.query_map([user],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    let resolved:std::collections::BTreeMap<_,_>=computed(crate::figures::splits::lookups(c,user,&venues_traded),&mut hash)?.map(|(_,named)|named.into_iter().collect()).unwrap_or_default();
    "names".hash(&mut hash);
    for entry in &resolved { read+=1; entry.hash(&mut hash); }
    let resolved=ids_source(resolved.values().flatten().copied());
    let bots="SELECT id FROM bots WHERE user_id=?1";
    // A split is matched through the listings of the venue each order was placed on (splits::groups), not only the
    // bot's venue now.
    let venues=format!("SELECT exchange_id FROM bots WHERE user_id=?1 UNION SELECT exchange_id FROM transactions WHERE bot_id IN ({bots})");
    // The settings' asset ids as db::bot and pair::load read them (allocations' keys, base_asset_ids, the pair's base, the quote).
    let settings="CASE WHEN json_valid(b.settings) THEN b.settings ELSE '{}' END";
    let named=format!("SELECT CAST(j.value AS INTEGER) FROM bots b, json_each({settings},'$.base_asset_ids') j WHERE b.user_id=?1 \
        UNION SELECT CAST(j.key AS INTEGER) FROM bots b, json_each({settings},'$.allocations') j WHERE b.user_id=?1 \
        UNION SELECT CAST(json_extract({settings},'$.quote_asset_id') AS INTEGER) FROM bots b WHERE b.user_id=?1 \
        UNION SELECT CAST(json_extract({settings},'$.base_asset_id') AS INTEGER) FROM bots b WHERE b.user_id=?1");
    // The account's tickers: a composition member's by its id (db::composition_minimum), and the listings on the
    // account's venues of what it trades or traded (db::tickers, db::pair_ticker, db::listings), or of a holding known only
    // by the name its orders were placed under (live::ticker_for_key).
    let traded=format!("{named} UNION SELECT asset_id FROM bot_index_assets WHERE bot_id IN ({bots}) UNION SELECT base_asset_id FROM transactions WHERE bot_id IN ({bots}){resolved}");
    // A holding recorded before its orders stored their asset is known by the name they were placed under: a scan of
    // the venue's listing, made only for those names.
    let spelled=format!("SELECT base FROM transactions WHERE bot_id IN ({bots}) AND base_asset_id IS NULL");
    let mut tickers=format!("SELECT ticker_id FROM bot_index_assets WHERE bot_id IN ({bots}) UNION SELECT id FROM tickers WHERE exchange_id IN ({venues}) AND base_asset_id IN ({traded})");
    if c.query_row(&format!("SELECT EXISTS({spelled})"),[user],|r|r.get(0))? { tickers+=&format!(" UNION SELECT id FROM tickers WHERE exchange_id IN ({venues}) AND base IN ({spelled})"); }
    // Ids read once, as literals: each query below that names them would otherwise read them again (the account's
    // ledger, three times over for the assets).
    let literal=|sql:&str|->Result<String,rusqlite::Error> {
        let ids=c.prepare(sql)?.query_map([user],|r|r.get::<_,Option<i64>>(0))?.filter_map(Result::transpose)
            .map(|id|id.map(|id|id.to_string())).collect::<Result<Vec<_>,_>>()?.join(",");
        Ok(if ids.is_empty(){"NULL".to_string()}else{ids})
    };
    let tickers=literal(&tickers)?;
    let assets=literal(&format!("SELECT base_asset_id FROM tickers WHERE id IN ({tickers}) UNION SELECT quote_asset_id FROM tickers WHERE id IN ({tickers}) \
        UNION {traded} UNION SELECT quote_asset_id FROM transactions WHERE bot_id IN ({bots}) \
        UNION SELECT asset_id FROM account_balances WHERE user_id=?1 \
        UNION SELECT base_asset_id FROM account_transactions WHERE user_id=?1 UNION SELECT asset_id FROM wash_sale_locks WHERE user_id=?1"))?;
    // A conversion reads an asset by its symbol alone, upper-cased (totals::exchange_rate, db::asset_by_symbol): any
    // asset under one of these symbols as stored or upper-cased, under USD (what the figures convert into), and under the
    // currency the account is shown in. By the symbol index: assets has no index on its category.
    let assets=format!("SELECT id FROM assets WHERE id IN ({assets}) UNION SELECT id FROM assets WHERE symbol IN (SELECT symbol FROM assets WHERE id IN ({assets}) \
        UNION SELECT upper(symbol) FROM assets WHERE id IN ({assets}) UNION SELECT 'USD' UNION SELECT upper(display_currency) FROM users WHERE id=?1)");
    let unread:&[&str]=&["updated_at","transient_data","ath","ath_updated_at","market_cap","market_cap_rank","circulating_supply"];
    let sets=[
        ("SELECT * FROM users WHERE id=?1".to_string(),"users"),
        ("SELECT * FROM bots WHERE user_id=?1 ORDER BY id".to_string(),"bots"),
        // Of a bot's transient data, only what web::figure::holdings reads: the operation flags, and whether it parses.
        ("SELECT id,json_valid(transient_data),CASE WHEN json_valid(transient_data) THEN json_array(json_extract(transient_data,'$.rebalance_pending'),\
          json_extract(transient_data,'$.liquidation_pending'),json_extract(transient_data,'$.liquidation_selling_since'),json_extract(transient_data,'$.redeploy_pending')) END \
          FROM bots WHERE user_id=?1 ORDER BY id".to_string(),"operations"),
        (format!("SELECT * FROM transactions WHERE bot_id IN ({bots}) ORDER BY id"),"transactions"),
        (format!("SELECT * FROM bot_index_assets WHERE bot_id IN ({bots}) ORDER BY id"),"bot_index_assets"),
        ("SELECT * FROM wash_sale_locks WHERE user_id=?1 ORDER BY id".to_string(),"wash_sale_locks"),
        (format!("SELECT * FROM exchanges WHERE id IN ({venues}) ORDER BY id"),"exchanges"),
        (format!("SELECT * FROM tickers WHERE id IN ({tickers}) ORDER BY id"),"tickers"),
        (format!("SELECT * FROM assets WHERE id IN ({assets}) ORDER BY id"),"assets"),
        // The ledger is read for its split rows (db::split_rows: every field of them) and for the assets its rows name
        // under each symbol (db::account_classes); a row edited in place into or out of either changes these.
        (format!("SELECT id,exchange_id,base_currency,base_asset_id,raw_data,transacted_at FROM account_transactions WHERE user_id=?1 AND entry_type={} \
                  AND CASE WHEN json_valid(raw_data) THEN json_extract(raw_data,'$.corporate_action') END='split' ORDER BY id",crate::figures::db::ADJUSTMENT),"splits"),
        ("SELECT DISTINCT base_currency,base_asset_id FROM account_transactions WHERE user_id=?1 ORDER BY 1,2".to_string(),"ledger classes"),
        ("SELECT DISTINCT asset_id FROM account_balances WHERE user_id=?1 ORDER BY 1".to_string(),"balances"),
        // An index reads every tradable listing of its venue in its quote (db::tickers): their coarsest quote precision
        // rounds its chart (chart::quote_decimals), and their tickers are what it asks the market for (`symbols`). Per
        // quote on each index's venue, one aggregate over the venue's index: their coarsest and finest precision, and
        // each one's id and ticker in id order. A held one's whole row is in the tickers above.
        ("SELECT exchange_id,quote_asset_id,min(quote_decimals),max(quote_decimals),group_concat(id||char(30)||ticker,char(31) ORDER BY id) FROM tickers \
          WHERE exchange_id IN (SELECT exchange_id FROM bots WHERE user_id=?1 AND type='Bots::DcaIndex') AND available=1 AND trading_enabled=1 \
          GROUP BY exchange_id,quote_asset_id ORDER BY 1,2".to_string(),"index listings"),
    ];
    for (sql,name) in sets {
        let mut statement=c.prepare(&sql)?;
        let kept:Vec<usize>=statement.column_names().iter().enumerate().filter(|(_,column)|!unread.contains(*column)).map(|(i,_)|i).collect();
        let user:&[&dyn rusqlite::ToSql]=&[&user];
        let mut rows=statement.query(&user[..statement.parameter_count()])?;
        name.hash(&mut hash);
        while let Some(row)=rows.next()? {
            read+=1;
            for &i in &kept {
                match row.get_ref(i)? {
                    ValueRef::Null=>0u8.hash(&mut hash),
                    ValueRef::Integer(v)=>(1u8,v).hash(&mut hash),
                    ValueRef::Real(v)=>(2u8,v.to_bits()).hash(&mut hash),
                    ValueRef::Text(v)|ValueRef::Blob(v)=>(3u8,v).hash(&mut hash),
                }
            }
        }
    }
    Ok((hash.finish(),read))
}
pub(crate) fn read_info(c:&Connection,app:&App,user:i64)->Result<Option<Info>,WebError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=read_info(&tx,app,user)?;tx.commit()?;return Ok(out)}
    let id:Option<i64>=c.query_row("SELECT k.id FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0 AND e.type='Exchanges::Alpaca' ORDER BY k.id LIMIT 1",[user],|r|r.get(0)).optional()?;
    let Some(id)=id else { return Ok(None) };
    // A failure to read the key is the read's failure: the caller's retry keeps the demand. Only an undecryptable key
    // means no figures (this install's configuration, not a passing fault).
    let columns=crate::sync::credential_columns(c,id)?;
    let credentials=match crate::sync::credentials_from_columns(&app.cipher,id,&columns) { Ok(c)=>c,Err(_)=>return Ok(None) };
    if !matches!(credentials.passphrase.as_deref(),None|Some("paper")) { return Ok(None); }
    let Some(version)=crate::engine::model::credential_version_by_id(c,id)? else{return Ok(None)};
    let identity=serde_json::json!([user,version.cache_stamp()]).to_string();
    let revision=revision(c,user)?.0;
    Ok(Some(Info{identity,revision,mark:mark(c)?,credentials,origin:version}))
}
pub(crate) struct Wire { real:ReqwestTransport,source:Source,origin:Option<crate::engine::model::CredentialVersion>,compute:Option<std::sync::Arc<tokio::sync::Semaphore>> }
impl Wire {
    pub(crate) fn with_origin(mut self,origin:crate::engine::model::CredentialVersion)->Self { self.origin=Some(origin);self }
    pub(crate) fn new(credentials:&Credentials,source:Source)->Self { Wire{real:ReqwestTransport::new(http::client(),credentials.key.clone(),credentials.secret.clone()).with_sensitive_values(credentials.redaction_values.clone()),source,origin:None,compute:None} }
    /// Each answer is settled (a pass over the whole body) off the runtime's thread, under the figures' compute permit.
    pub(crate) fn settled_under(mut self,compute:std::sync::Arc<tokio::sync::Semaphore>)->Self { self.compute=Some(compute);self }
    /// A live answer as `ReqwestTransport::settle` settles it (checked and scrubbed); a scripted one scrubbed as a live
    /// body is. Off the runtime's thread when the fill says so (`settled_under`).
    async fn settle(&self,reply:HttpResponse,live:bool)->Result<HttpResponse,TransportError> {
        let real=self.real.clone();
        let work=move||if live { real.settle(reply) } else { HttpResponse{status:reply.status,body:real.redact_diagnostic(&reply.body)} };
        let Some(compute)=&self.compute else { return Ok(work()) };
        let permit=compute.clone().acquire_owned().await.map_err(|_|TransportError::Permanent("Market data unavailable".into()))?;
        tokio::task::spawn_blocking(move||{ let _permit=permit; work() }).await.map_err(|_|TransportError::Permanent("Market data unavailable".into()))
    }
}
impl Transport for Wire {
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{self.origin.clone()}
    fn redact_diagnostic(&self,text:&str)->String{self.real.redact_diagnostic(text)}
    async fn send(&self,r:&HttpRequest)->Result<HttpResponse,TransportError> {
        match &self.source {
            Source::Live=>self.settle(self.real.send_unsettled(r,http::MAX_BODY).await?,true).await,
            Source::Disabled=>Err(TransportError::Permanent("Market data unavailable".into())),
            Source::Script(script)=>{
                // Market data is scripted by the parameters that pick a series; any other host by its whole sorted query.
                let host=r.base.split_once("://").map_or(r.base.as_str(),|(_,h)|h);
                let data=host=="data.alpaca.markets";
                let mut picked:Vec<_>=r.query.iter().filter(|(k,_)|!data||r.path.ends_with("/bars")&&(*k=="adjustment"||*k=="symbols")).collect();
                if !data{picked.sort();}
                let suffix=if picked.is_empty(){String::new()}else{format!("?{}",picked.iter().map(|(k,v)|format!("{k}={v}")).collect::<Vec<_>>().join("&"))};
                let key=format!("{} {host}{}{suffix}",r.method,r.path);
                let reply=script.get(&key).ok_or_else(||TransportError::Permanent("Unscripted market request".into()))?;
                if reply["network"].is_string(){return Err(TransportError::NotSent("Market data unavailable".into()));}
                // A test holds an answer back to keep a fill in flight; the live source never reads a script.
                if let Some(ms)=reply["delay_ms"].as_u64(){tokio::time::sleep(Duration::from_millis(ms)).await;}
                self.settle(HttpResponse{status:reply["status"].as_u64().unwrap_or(200) as u16,body:reply["body"].as_str().map(str::to_string).unwrap_or_else(||reply["body"].to_string())},false).await
            }
        }
    }
    async fn send_limited(&self,r:&HttpRequest,limit:usize)->Result<HttpResponse,TransportError>{
        if matches!(self.source,Source::Live){return self.settle(self.real.send_unsettled(r,limit).await?,true).await;}
        let reply=self.send(r).await?;
        if reply.body.len()>limit{Err(TransportError::Permanent("Market data unavailable".into()))}else{Ok(reply)}
    }
}
pub fn symbols(c:&Connection,user:i64)->Result<Vec<String>,crate::figures::FiguresError>{
    let mut names=std::collections::BTreeSet::new();
    for (id,_) in db::account_bots(c,user)? {
        let kind:String=c.query_row("SELECT type FROM bots WHERE id=?1",[id],|r|r.get(0))?;
        if matches!(kind.as_str(),"Bots::DcaSingleAsset"|"Bots::Signal"){
            for ticker in crate::figures::pair::load(c,id)?.tickers{names.insert(ticker.ticker);}
            continue;
        }
        let bot=db::bot(c,id)?;
        for ticker in db::tickers(c,&bot)? { names.insert(ticker.ticker); }
    }
    Ok(names.into_iter().collect())
}
/// Figures CPU work on the builder's own connection, off the runtime's thread, holding the one compute permit
/// (`Service::compute`) until it returns, never on the connection pages use (`App::db`). An in-memory database has no
/// second connection: there the work runs on the page's, still under the permit.
async fn build<T:Send+'static>(app:&App,work:impl FnOnce(&Connection)->Result<T,WebError>+Send+'static)->Result<T,WebError> {
    let permit=app.figure_service.compute().acquire_owned().await.map_err(|e|WebError::Task(e.to_string()))?;
    let path=match app.figure_service.builder_path() {
        Some(path)=>path,
        None=>{ let path=app.db(|c|Ok(c.path().unwrap_or_default().to_string())).await?; app.figure_service.set_builder_path(path.clone()); path }
    };
    if path.is_empty() { return app.db(move|c|{ let _permit=permit; work(c) }).await; }
    let service=app.figure_service.clone();
    tokio::task::spawn_blocking(move||{ let _permit=permit; service.with_builder(work) }).await.map_err(|e|WebError::Task(e.to_string()))?
}
/// Figures rendered for kept use, by (locale, path prefix).
type Rendered=Vec<((String,String),(Value,super::Slots))>;
/// The key a publication renders: the account's own locale and its paths (Bot#with_user_locale,
/// ApplicationController#default_url_options).
pub(crate) fn own_key(c:&Connection,user:i64)->Result<(String,String),rusqlite::Error> {
    let chosen:Option<String>=c.query_row("SELECT locale FROM users WHERE id=?1",[user],|r|r.get(0)).optional()?.flatten();
    let locale=crate::web::locale::switch(None,chosen.as_deref());
    Ok((locale.to_string(),crate::web::locale::path(locale,"")))
}
fn kept_key(revision:u64,at:At,locale:&str,prefix:&str)->String { serde_json::json!([revision,at.0,locale,prefix]).to_string() }
fn kept_value((value,slots):(Value,super::Slots))->std::sync::Arc<Value> { std::sync::Arc::new(serde_json::json!({"figures":value,"slots":slots})) }
/// The fill's computation: its last pass, with every market answer it needs, renders the account's own key and every key
/// of the work it carries, to be kept with the cache (`spawn_fill`).
async fn fill(app:&App,user:i64,cache:&mut Cache,wire:&Wire,now:At,keys:std::collections::BTreeSet<(String,String)>)->Result<Rendered,Option<&'static str>> {
    fill_result(app,user,cache,wire,now,|_,_|Ok(true),move|c,reader|{
        let mut keys=keys.clone();
        keys.insert(own_key(c,user)?);
        let mut out=vec![];
        for (locale,prefix) in keys {
            let rendered=super::kept_account(c,user,reader,now,&locale,&prefix)?;
            // A pass that still lacks market data fetches it first; only the last pass renders every key.
            if reader.failed() || !reader.demands().is_empty() { return Ok(vec![]); }
            out.push(((locale,prefix),rendered));
        }
        Ok(out)
    }).await
}
/// The earliest end (Unix seconds) of a wash-sale lock the account's figures show, after `now`: a held asset's row moves
/// out of the locked table then. Only where the rule applies (holdings::render: enabled, and US, GB or IE, US when blank).
fn lock_deadline(c:&Connection,user:i64,now:At)->Result<Option<i64>,WebError> {
    let (enabled,jurisdiction):(Option<bool>,Option<String>)=c.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1",[user],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let jurisdiction=jurisdiction.as_deref().filter(|j|!j.trim().is_empty()).unwrap_or("US");
    if enabled!=Some(true) || !["US","GB","IE"].contains(&jurisdiction) { return Ok(None); }
    let mut statement=c.prepare("SELECT buy_locked_until FROM wash_sale_locks WHERE user_id=?1 AND buy_locked_until IS NOT NULL")?;
    let mut earliest:Option<At>=None;
    for until in statement.query_map([user],|r|r.get::<_,String>(0))? {
        // An unreadable deadline ends the figures at once: the next read refills.
        let until=At::from_sql(&until?).unwrap_or(now);
        if until>now { earliest=Some(earliest.map_or(until,|e|e.min(until))); }
    }
    Ok(earliest.map(|at|at.utc().timestamp()))
}
/// Why a failed account's figures are absent, from its stored fills alone: an executed fill whose value is unknown.
fn walk_reason(c:&Connection,user:i64)->Option<&'static str> {
    let fills=budget::within(||->Result<(),crate::figures::FiguresError>{
        for (id,_) in db::account_bots(c,user)?{for order in db::orders(c,id)?{crate::figures::fill::parse(&order)?;}}
        Ok(())
    });
    matches!(fills,Err(crate::figures::FiguresError::NotComputed(ref why)) if why=="executed fill value unavailable").then_some("executed fill value unavailable")
}
/// What a render-only pass came to.
enum Kept { Done, Overtaken, Failed(String) }
/// The render-only pass: keys this ready cache has not kept are rendered from it, with no market request, each under a
/// figure's budget of its own, and kept if no write overtook the pass. `Overtaken`: the revision moved, and a retry fills
/// anew. `Failed`: a key could not be computed against the unchanged revision, which a retry would only repeat.
fn keep(c:&Connection,user:i64,cache:&Cache,at:At,prepared:u64,keys:&std::collections::BTreeSet<(String,String)>)->Kept {
    let missing:Vec<&(String,String)>=keys.iter().filter(|(locale,prefix)|cache.kept(&kept_key(prepared,at,locale,prefix)).is_none()).collect();
    if missing.is_empty() { return Kept::Done; }
    let names=match symbols(c,user) { Ok(names)=>names, Err(e)=>return Kept::Failed(format!("{e:?}")) };
    let (mut out,mut failed)=(vec![],vec![]);
    for (locale,prefix) in missing {
        let (rendered,_)=budget::scope(budget::FIGURE,||{
            let reader=Reader::new(cache,at.utc().timestamp()).with_current(c).with_symbols(names.clone());
            let rendered=super::kept_account(c,user,&reader,at,locale,prefix)?;
            if reader.failed() || !reader.demands().is_empty() { return Err(crate::figures::FiguresError::NotComputed("Market data unavailable".into())); }
            Ok(rendered)
        });
        match rendered { Ok(rendered)=>out.push((kept_key(prepared,at,locale,prefix),rendered)), Err(e)=>failed.push(format!("{locale}: {e:?}")) }
    }
    match revision(c,user) {
        Ok((now,_)) if now!=prepared=>Kept::Overtaken,
        Ok(_)=>{ for (key,rendered) in out { cache.keep(key,kept_value(rendered)); } if failed.is_empty() { Kept::Done } else { Kept::Failed(failed.join("; ")) } }
        Err(e)=>Kept::Failed(format!("{e:?}")),
    }
}
/// The MCP caller needs only readiness; the page caller retains a safe reason from computation.
pub(crate) async fn fill_with(app:&App,user:i64,cache:&mut Cache,wire:&Wire,now:At,admit:fn(&Connection,i64)->Result<bool,WebError>,work:impl Fn(&Connection,&Reader<'_>)->bool+Clone+Send+'static)->bool {
    fill_result(app,user,cache,wire,now,admit,move|c,reader|if work(c,reader){Ok(())}else{Err(crate::figures::FiguresError::NotComputed("Market data unavailable".into()))}).await.is_ok()
}
/// Compute, fetch demands and retry at most four times. Only a fixed public reason crosses into
/// failed snapshots: arbitrary provider diagnostics and stored data cannot become page text.
async fn fill_result<T:Send+'static>(app:&App,user:i64,cache:&mut Cache,wire:&Wire,now:At,admit:fn(&Connection,i64)->Result<bool,WebError>,work:impl Fn(&Connection,&Reader<'_>)->Result<T,crate::figures::FiguresError>+Clone+Send+'static)->Result<T,Option<&'static str>> {
    for _ in 0..4 {
        let snapshot=cache.clone();let work=work.clone();
        let pass=build(app,move|c|{
            if !admit(c,user)? { return Ok(None); }
            let result=budget::within(||{
                let names=symbols(c,user)?;
                let reader=Reader::new(&snapshot,now.utc().timestamp()).with_current(c).with_symbols(names);
                let result=work(c,&reader);
                let failed=reader.failed();
                let (out,reason)=match result {
                    Ok(out)=>(Some(out).filter(|_|!failed),None),
                    Err(crate::figures::FiguresError::NotComputed(reason)) if reason=="Currency conversion unavailable"=>(None,Some("Currency conversion unavailable")),
                    Err(_)=>(None,None),
                };
                Ok::<_,crate::figures::FiguresError>((reader.demands(),out,reason))
            });
            Ok(result.ok()) // allow-swallow: computation failure becomes a failed fill; only the safe reason above is retained
        }).await;
        let Ok(Some((demands,out,reason)))=pass else { return Err(None) };
        if demands.is_empty(){return out.ok_or(reason);}
        if demands.len()>crate::figures::page_market::MAX_ENTRIES{return Err(None);}
        cache.fill_bounded(wire,demands,now.utc().timestamp(),&app.figure_service.compute()).await;
    }
    Err(None)
}
pub async fn prepare(app:&App,user:i64)->Result<Snapshot,WebError>{
    begin(app,user).await
}
/// Boxed, so that its type is known: the end of a fill publishes, and publishing begins a fill.
type Begun<'a>=std::pin::Pin<Box<dyn std::future::Future<Output=Result<Snapshot,WebError>>+Send+'a>>;
fn begin(app:&App,user:i64)->Begun<'_>{ Box::pin(async move {
    if matches!(app.figure_source,Source::Disabled){return Ok(Snapshot::Cold);}
    let inner=app.clone();
    // No paper Alpaca key, so no fill: why its figures are absent was read by the queue (`consume`) against the same
    // fingerprint the figures are held to; until it is, the page shows the loading state and asks for it.
    let Some(info)=app.db(move|c|read_info(c,&inner,user)).await? else{
        let revision=app.db(move|c|Ok(revision(c,user)?.0)).await?;
        return Ok(app.figure_service.unkeyed(user,revision,app.now().timestamp()).map_or(Snapshot::Cold,Snapshot::Failed));
    };
    let Some(now)=At::from_utc(app.now()) else{return Ok(Snapshot::Failed(None))};
    match app.figure_service.begin(user,&info.identity,info.revision,now.utc().timestamp()) {
        Load::Cold=>Ok(Snapshot::Cold),Load::Failed(reason)=>Ok(Snapshot::Failed(reason)),
        // Other accounts hold both fills: the demand is kept, and the end of one of theirs serves it.
        Load::Busy=>{app.figure_service.want(user);Ok(Snapshot::Cold)},
        Load::Ready(cache,_,until)=>{let at=cache.stamp().unwrap_or(now);Ok(Snapshot::Ready(cache,at,info.revision,Some(info.mark),until))},
        Load::Start(ticket,cache)=>{ spawn_fill(app,user,info,now,ticket,cache,None); Ok(Snapshot::Cold) }
    }
})}
/// A fill of this account's market cache, on a task of its own. `work`: what the queue asked of it (`follow`).
fn spawn_fill(app:&App,user:i64,info:Info,now:At,ticket:super::service::Ticket,mut cache:Cache,work:Option<Work>) {
    let inner=app.clone();let source=app.figure_source.clone();
    tokio::spawn(async move {
        let wire=Wire::new(&info.credentials,source).with_origin(info.origin.clone()).settled_under(inner.figure_service.compute());
        let keys=work.as_ref().map(|work|work.locales.clone()).unwrap_or_default();
        // Past its deadline the fill stops waiting; a worker it started finishes on its own, holding the compute permit.
        let timed=tokio::time::timeout(inner.figure_service.fill_timeout(),fill(&inner,user,&mut cache,&wire,now,keys)).await;
        let timed_out=timed.is_err();
        let result=timed.unwrap_or(Err(None));
        let check=inner.clone();
        let current=inner.db(move|c|read_info(c,&check,user)).await;
        let unchanged=matches!(current,Ok(Some(ref current)) if crate::engine::model::current_for(Some(&info.origin),Some(&current.origin)).is_fresh() && current.revision==info.revision);
        cache.set_stamp(now);
        let (ready,reason,deadline)=match result {
            // What the last pass rendered is kept with the cache: no page computes it.
            Ok(rendered) if unchanged=>match build(&inner,move|c|lock_deadline(c,user,now)).await {
                Ok(deadline)=>{ for ((locale,prefix),rendered) in rendered { cache.keep(kept_key(info.revision,now,&locale,&prefix),kept_value(rendered)); } (true,None,deadline) }
                Err(_)=>(false,None,None), // a lifetime that cannot be read is no lifetime: the next read fills again
            },
            Err(Some(reason)) if unchanged=>(false,Some(reason),None),
            // Stored at once: no CPU work (which would wait for the worker still running) before the failure is known.
            Err(None) if timed_out=>(false,None,None),
            _=>(false,build(&inner,move|c|Ok(walk_reason(c,user))).await.unwrap_or(None),None), // allow-swallow: an unread reason shows the plain no value
        };
        ticket.finish_until(cache,ready,reason,inner.now().timestamp(),deadline);
        // Whoever waits and has no fill running hears the end of this one: this account, or one the two-fill
        // limit turned away. A fill a write overtook is not published: publishing starts the next fill, and each
        // fill a waiting account starts spends one of its own allowance (`service::ALLOWANCE`), so failures and
        // writes end in a publication, not a loop. Waiting behind another account spends nothing.
        // ponytail: every waiting account per fill end; one queue of accounts if installs grow past a handful.
        // A publication that fails here (a transient database error) is handed to the `figures` service, which tries
        // it again with its backoff (`follow`).
        let mut publish:std::collections::BTreeSet<i64>=inner.figure_service.waiting_idle().into_iter().collect();
        match work {
            // A write overtook this fill: its work goes back on the queue, which starts the next fill with it.
            Some(work) if !unchanged=>inner.figure_service.enqueue(user,Work{not_before:inner.now().timestamp(),..work}),
            Some(work) if work.publish=>{ publish.insert(user); }
            _=>{}
        }
        for waiting in publish {
            if publish_with(inner.clone(),waiting).await.is_err() { inner.figure_service.mark_owner(waiting); }
        }
    });
}
/// Kept figures with the request's form token written where the renderer left each form's empty token
/// (`figure::kept_account`), and nowhere else: stored text is never searched.
fn with_token(figures:&Value,slots:&super::Slots,csrf:&str)->Value {
    let mut out=figures.clone();
    for (bot,offsets) in slots {
        if let Some(Value::String(metrics))=out.get_mut("bots").and_then(|bots|bots.get_mut(bot)).and_then(|bot|bot.get_mut("metrics")) {
            *metrics=super::holdings::with_token(metrics,offsets,csrf);
        }
    }
    out
}
/// Converts a terminal computation failure into absence at every dependent presentation target.
/// `Ok(None)` while the figures are cold; an error only when the fallback cannot read the account's bots.
pub fn render(c:&Connection,user:i64,snapshot:&Snapshot,locale:&str,csrf:&str,prefix:&str,clock:&dyn crate::engine::Clock)->Result<Option<Value>,WebError>{
    let reason=match snapshot {
        Snapshot::Cold=>return Ok(None),
        Snapshot::Failed(reason)=>*reason,
        Snapshot::Ready(cache,now,prepared_revision,read,until)=>{
            let current=cache.producer().as_ref().is_some_and(|origin|crate::engine::model::credential_is_current(c,origin).unwrap_or(false));
            // The fingerprint is the most expensive read of a warm page, and `prepare` read it a moment ago on this
            // connection (App::db's one): read again only if anything was written since.
            let unchanged=read.is_some_and(|read|mark(c).ok()==Some(read));
            if current && (unchanged || revision(c,user).ok().map(|(revision,_)|revision)==Some(*prepared_revision)) {
                // The figures are a function of this cache, the revision and `now` (the cache's stamp), and of how they
                // are rendered. The fill or the queue renders them; a page only reads what was kept, and a form token is
                // per request: kept figures record where each form's field goes, and a page writes its own.
                let Some(kept)=cache.kept(&kept_key(*prepared_revision,*now,locale,prefix)) else { return Ok(None) };
                let Ok(slots)=serde_json::from_value::<super::Slots>(kept["slots"].clone()) else { return Ok(None) };
                let shown=with_token(&kept["figures"],&slots,csrf);
                // Their lifetime is checked as they are shown, not when the page prepared them: past the cut (or a wash-sale
                // lock's end) the page shows the loading state and the queue fills again.
                if clock.now().timestamp()>=*until { return Ok(None); }
                return Ok(Some(shown));
            }
            None
        }
    };
    let no_value=match reason {
        Some("Currency conversion unavailable")=>"<span class=\"no-value\">Currency conversion unavailable</span>",
        Some("executed fill value unavailable")=>"<span class=\"no-value\">Figures unavailable: executed fill value unavailable</span>",
        _=>super::NO_VALUE,
    };
    // The fallback is every bot's no value, or nothing: a failure to read the bots fails the render, never publishes the
    // headline alone. Read as `db::account_bots` reads them, outside any figure's budget.
    let mut bots=serde_json::Map::new();
    let mut statement=c.prepare("SELECT id, type FROM bots WHERE user_id = ?1 AND status != ?2")?;
    for row in statement.query_map(rusqlite::params![user,crate::enums::BotStatus::Deleted as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?)))? {
        let (id,kind)=row?;
        let kind=if kind.as_deref()==Some("Bots::DcaIndex"){"dca_index"}else{"dca_multi_asset"};
        bots.insert(id.to_string(),serde_json::json!({"tile":format!("<div id=\"pnl_bots_{kind}_{id}\">{}</div>",no_value),"metrics":format!("<div id=\"metrics\">{}</div>",no_value),"chart":format!("<div id=\"chart\">{}</div>",no_value)}));
    }
    Ok(Some(serde_json::json!({"bots":bots,"account":format!("<div id=\"global-pnl\">{}</div>",no_value)})))
}
/// What a page shows: the kept figures, or None (Rails' loading state) while they are not kept, and then the queue makes
/// and publishes them in the page's locale and paths.
pub fn render_page(app:&App,c:&Connection,user:i64,snapshot:&Snapshot,locale:&str,csrf:&str,prefix:&str)->Result<Option<Value>,WebError>{
    let shown=render(c,user,snapshot,locale,csrf,prefix,&*app.clock)?;
    if shown.is_none() && !matches!(app.figure_source,Source::Disabled) { enqueue_page(app,user,locale,prefix); }
    Ok(shown)
}
/// Puts the user's figures on their streams (`figure::streams`): every tile, each bot page's metrics and chart, and the
/// account headline. Each is rendered as Rails renders a broadcast: in the user's locale and under its paths
/// (Bot#with_user_locale, ApplicationController#default_url_options), with no token in a form (the renderer has no
/// session). Rails broadcasts what was asked for; one publication carries every target, each payload Rails' own.
/// While a fill is in flight nothing is published: the user is noted as waiting, and the fill's end publishes.
pub async fn publish(app:&App,user:i64)->Result<(),WebError>{
    publish_with(app.clone(),user).await.map(|_|())
}
/// `true` once a publication went out; `false` when the account was left waiting (a fill runs, or the figures were
/// overtaken), for the end of a fill, or the caller, to publish.
async fn publish_with(app:App,user:i64)->Result<bool,WebError>{
    // Ready figures are held against the account as it is once rendered, just before delivery: a publication whose
    // figures expired, were written or were refilled meanwhile delivers nothing, and is tried once more, which finds them
    // ready anew or starts a fill whose end publishes.
    for _ in 0..2 {
        let snapshot=begin(&app,user).await?;
        if matches!(snapshot,Snapshot::Cold){app.figure_service.want(user);return Ok(false);}
        let serial=app.figure_service.settled(user);
        let ready=matches!(snapshot,Snapshot::Ready(..));
        let inner=app.clone();
        let (streams,info,unkept,expired)=app.db(move|c|{
            let key=own_key(c,user)?;
            let expired=matches!(&snapshot,Snapshot::Ready(..,until) if inner.now().timestamp()>=*until);
            let (streams,unkept)=match render(c,user,&snapshot,&key.0,"",&key.1,&*inner.clock)? {
                // A failure here fails the publication: nothing is recorded or delivered, and the caller's retry keeps the demand.
                Some(rendered)=>(super::streams(c,user,&rendered).map_err(|e|WebError::Task(format!("figure streams: {e:?}")))?,None),
                None=>(vec![],(ready&&!expired).then_some(key)),
            };
            let info=if ready { read_info(c,&inner,user)? } else { None };
            Ok((streams,info,unkept,expired))
        }).await?;
        // Expired while it was being rendered: tried once more, which fills anew.
        if expired { continue; }
        // Ready, and nothing kept in the account's own key: the queue renders it from this cache and publishes.
        if let Some(key)=unkept { app.figure_service.enqueue(user,Work{publish:true,locales:[key].into(),not_before:app.now().timestamp()}); return Ok(false); }
        if ready {
            let fresh=info.is_some_and(|info|app.figure_service.record(user,serial,&info.identity,info.revision,app.now().timestamp(),&streams));
            if !fresh { continue; }
        }
        app.figure_service.served(user);
        deliver(&app,streams).await;
        return Ok(true);
    }
    // Overtaken twice: the account waits, and the end of the next fill publishes.
    app.figure_service.want(user);
    Ok(false)
}

/// Puts payloads on the hub. Each connection's mailbox keeps the latest payload of every target (`cable::Mailbox`), so
/// a large account's publication neither waits on a slow connection nor pushes a healthy one off.
pub async fn deliver(app:&App,streams:Vec<(String,String)>){
    for (stream,html) in streams { app.hub.broadcast(&stream,&html); }
}

/// What a connection that (re)subscribes to one of `user`'s figure streams is sent: the account's latest publication on
/// it while its figures are fresh (`Service::latest`). Otherwise nothing, and one publication is asked for, as a page
/// asks (`publish`): ready figures go out at once, and otherwise the account waits, through the fill and allowance
/// machinery, and the fill's end publishes. So a page that stays mounted converges after any reconnect: after a
/// dropped publication, after its figures expired or were written, after the account was evicted, after a restart.
/// An account with no figures (no paper Alpaca key, or market data disabled) is sent and asks nothing.
/// The account's latest publication on `stream` while its figures are fresh (`Service::latest`), asking nothing.
pub async fn latest(app:&App,user:i64,stream:&str)->Option<Vec<std::sync::Arc<str>>>{
    let inner=app.clone();
    let info=app.db(move|c|read_info(c,&inner,user)).await.ok()??;
    app.figure_service.latest(user,&info.identity,info.revision,app.now().timestamp(),stream)
}
/// A page in `locale` under `prefix` found no figures to show: they are made and published by the queue.
pub fn enqueue_page(app:&App,user:i64,locale:&str,prefix:&str) {
    app.figure_service.enqueue(user,Work{publish:true,locales:[(locale.to_string(),prefix.to_string())].into(),not_before:app.now().timestamp()});
}
/// One account's queued work (`follow`). A ready entry renders the keys it lacks from its own cache, with no market
/// request; otherwise a fill starts and carries the work, or the work waits for the fill that runs (`Service::enqueue`
/// parks it) or for capacity.
async fn consume(app:&App,user:i64,mut work:Work) {
    if matches!(app.figure_source,Source::Disabled){return;}
    let inner=app.clone();
    let info=match app.db(move|c|read_info(c,&inner,user)).await {
        Ok(Some(info))=>info,
        // No paper key, so no figures: why, from the stored fills, for the page to read (`begin`), and published if asked.
        Ok(None)=>{
            let now=app.now().timestamp();
            let read=build(app,move|c|Ok((revision(c,user)?.0,walk_reason(c,user)))).await;
            // The builder could not read: tried again a second on, as a failed account read is.
            let Ok((revision,reason))=read else { work.not_before=app.now().timestamp().saturating_add(1); app.figure_service.enqueue(user,work); return; };
            app.figure_service.set_unkeyed(user,revision,now.saturating_add(300-now.rem_euclid(300)),reason);
            // A write after the fingerprint was read, or the cut while the reason was read: the page would find it cold,
            // and no fill ends to publish for this account. The work is queued again and reads it anew.
            match publish_with(app.clone(),user).await { Ok(false) if work.publish=>app.figure_service.enqueue(user,work), Err(_) if work.publish=>app.figure_service.mark_owner(user), _=>{} }
            return;
        }
        Err(_)=>{ work.not_before=app.now().timestamp().saturating_add(1); app.figure_service.enqueue(user,work); return; }
    };
    let Some(now)=At::from_utc(app.now()) else{return};
    match app.figure_service.begin(user,&info.identity,info.revision,now.utc().timestamp()) {
        Load::Ready(cache,_,_)=>{
            let at=cache.stamp().unwrap_or(now);
            let (publish,mut keys,revision)=(work.publish,work.locales.clone(),info.revision);
            match build(app,move|c|{ if publish { keys.insert(own_key(c,user)?); } Ok(keep(c,user,&cache,at,revision,&keys)) }).await {
                // Published only once its own key is kept: a publication never computes.
                Ok(Kept::Done)=>if publish && publish_with(app.clone(),user).await.is_err() { app.figure_service.mark_owner(user); },
                // A write overtook the pass: the work is queued again. Its retry finds the new revision and starts a fill
                // that carries it, and a fill a write overtakes queues it again too (`spawn_fill`): the work ends in a fill
                // that no write overtook.
                Ok(Kept::Overtaken)=>{ work.not_before=now.utc().timestamp(); app.figure_service.enqueue(user,work); }
                // Nothing moved, so a retry would compute the same: said once, and left to the next fill (a write, the cut).
                Ok(Kept::Failed(why))=>app.figure_service.note(&format!("figures: account {user}: keys not rendered from its ready figures: {why}")),
                // The builder could not run: tried again a second on.
                Err(_)=>{ work.not_before=now.utc().timestamp().saturating_add(1); app.figure_service.enqueue(user,work); }
            }
        }
        Load::Start(ticket,cache)=>spawn_fill(app,user,info,now,ticket,cache,Some(work)),
        Load::Cold=>app.figure_service.enqueue(user,work),
        Load::Busy=>{ work.not_before=now.utc().timestamp().saturating_add(1); app.figure_service.enqueue(user,work); }
        Load::Failed(_)=>if work.publish && publish_with(app.clone(),user).await.is_err() { app.figure_service.mark_owner(user); },
    }
}
pub async fn resubscribed(app:&App,user:i64,stream:&str)->Vec<std::sync::Arc<str>>{
    if matches!(app.figure_source,Source::Disabled){return Vec::new();}
    let inner=app.clone();
    // A failure to read the account (a transient database error) asks for the publication through the retry path.
    let info=match app.db(move|c|read_info(c,&inner,user)).await { Ok(info)=>info, Err(_)=>{ app.figure_service.mark_owner(user); return Vec::new(); } };
    let Some(info)=info else { return Vec::new() };
    if let Some(kept)=app.figure_service.latest(user,&info.identity,info.revision,app.now().timestamp(),stream) { return kept; }
    let app=app.clone();
    tokio::spawn(async move { if publish_with(app.clone(),user).await.is_err() { app.figure_service.mark_owner(user); } });
    Vec::new()
}

/// How long the `figures` service gathers marks before it publishes: a basket's legs and a sweep's fills come together.
const COALESCE:Duration=Duration::from_millis(500);
/// The longest the `figures` service sleeps before it looks at the queue's clock again.
const QUEUE_POLL:Duration=Duration::from_secs(1);
/// A publication that failed (a transient database error) is tried again after `BACKOFF`, doubling, at most `RETRIES`
/// times in a row: about eight seconds in all.
const BACKOFF:Duration=Duration::from_millis(250);
const RETRIES:u32=5;
/// The `figures` service of `deltabadger serve`: an order the engine recorded or wrote changes its bot's figures, and
/// Rails then broadcasts them (Bot::UpdateMetricsJob). Two parts in one future. One reads the engine's queue as fast as
/// it fills and only marks the bot (`Service::mark`: O(1), one mark per bot, never waiting), so nothing accumulates
/// there while a publication runs. The other wakes on a mark, gathers the burst for `COALESCE`, and publishes each
/// owner once. A write moved the revision, so a publication starts a fill and its end publishes. Returns once a stop is
/// requested; an engine that went away leaves it waiting for that stop.
pub async fn follow(app:App,mut events:tokio::sync::mpsc::UnboundedReceiver<crate::engine::events::EngineEvent>,mut stop:tokio::sync::watch::Receiver<bool>)->Result<(),String>{
    use crate::engine::events::EngineEvent;
    let service=app.figure_service.clone();
    let marking=async {
        while let Some(event)=events.recv().await {
            if let EngineEvent::OrderRecorded{bot_id,..}|EngineEvent::OrderUpdated{bot_id,..}=event { service.mark(bot_id); }
        }
    };
    let publishing=async {
        let mut failures=0u32;
        let mut tick=super::warm::next_tick(app.now().timestamp());
        loop {
            if app.now().timestamp()>=tick { super::warm::tick(&app).await; tick=super::warm::next_tick(app.now().timestamp()); }
            // Woken by a mark, by queued work, or by the earliest queued work falling due.
            // ponytail: the clock is looked at once a second while nothing else wakes this; a clock-change signal if that matters.
            let due=service.next_due().map_or(tick,|due|due.min(tick));
            let wait=Duration::from_secs(due.saturating_sub(app.now().timestamp()).max(0) as u64).min(QUEUE_POLL);
            if !wait.is_zero() { tokio::select!{ _=service.marked_wait()=>{}, _=tokio::time::sleep(wait)=>{} } }
            for (user,work) in service.take_due(app.now().timestamp()) { consume(&app,user,work).await; }
            if service.marked()==0 && !service.has_owners() { continue; }
            tokio::time::sleep(COALESCE).await;
            let bots=service.take_marked();
            let mut owners=service.take_owners();
            if bots.is_empty() && owners.is_empty() { continue; }
            // A failed lookup or publication keeps its bots or its account marked: they are tried again after a backoff.
            let mut failed_bots=Vec::new();
            if !bots.is_empty() {
                let ids=serde_json::json!(bots).to_string();
                let found=app.db(move|c|{
                    let mut s=c.prepare("SELECT DISTINCT user_id FROM bots WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY user_id")?;
                    let users=s.query_map([ids],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
                    Ok(users)
                }).await;
                match found { Ok(users)=>owners.extend(users), Err(_)=>failed_bots.extend(bots) }
            }
            let mut failed_owners=Vec::new();
            for user in owners { if publish(&app,user).await.is_err() { failed_owners.push(user); } }
            if failed_bots.is_empty() && failed_owners.is_empty() { failures=0; continue; }
            failures+=1;
            // ponytail: RETRIES attempts, then the demand waits for the next order or page request, as Rails' job would.
            if failures>RETRIES { failures=0; continue; }
            tokio::time::sleep(BACKOFF*2u32.pow(failures-1)).await;
            for bot in failed_bots { service.mark(bot); }
            for user in failed_owners { service.mark_owner(user); }
        }
    };
    let engine_gone=tokio::select!{
        _=stop.wait_for(|stopped|*stopped)=>false,
        _=marking=>true,
        _=publishing=>false,
    };
    if engine_gone { let _=stop.wait_for(|stopped|*stopped).await; } // allow-swallow: an error means no stop can come, and returning is the stop
    Ok(())
}

#[cfg(test)]
mod credential_cache_tests {
    use super::*;
    use crate::{web::Config,engine::FixedClock,venue::http::ScriptedTransport};
    #[tokio::test]
    async fn r_each_credential_column_starts_figures_with_an_empty_market_cache() {
        let dir=tempfile::tempdir().unwrap();let file=dir.path().join("primary.sqlite3");
        let c=Connection::open(&file).unwrap();c.execute_batch(include_str!("../../../tests/fixtures/settings_primary_schema.sql")).unwrap();
        c.execute("INSERT INTO users(email,encrypted_password,created_at,updated_at)VALUES('r@example.com','x','2026-01-01','2026-01-01')",[]).unwrap();
        c.execute("INSERT INTO exchanges(type,name,created_at,updated_at)VALUES('Exchanges::Alpaca','Alpaca','2026-01-01','2026-01-01')",[]).unwrap();
        let now="2026-09-10T12:00:00Z".parse().unwrap();
        let env=|name:&str|(name=="SECRET_KEY_BASE").then(||"settings-cache-test-secret".to_string());
        let app=App::new(Config::from_env(&env).unwrap(),&env,Connection::open(&file).unwrap(),std::sync::Arc::new(FixedClock(now))).unwrap();
        c.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,passphrase,created_at,updated_at)VALUES(1,1,0,1,?1,?2,?3,'2026-01-01','2026-01-01')",(app.cipher.encrypt("a"),app.cipher.encrypt("a-secret"),app.cipher.encrypt("paper"))).unwrap();
        let columns=["key","secret","passphrase","access_token","rsa_signature_key","rsa_encryption_key","dh_param"];
        let request=HttpRequest{method:"GET",base:"https://data.alpaca.markets".into(),path:"/v2/stocks/quotes/latest".into(),query:vec![],body:None,not_after:None};
        let wire=ScriptedTransport::from_script(&serde_json::json!({"GET /v2/stocks/quotes/latest":[{"status":200,"body":{"quotes":{"AAA":{"ap":10000}}}}]}));
        for column in columns {
            let service=crate::web::figure::service::Service::default();
            let info=read_info(&c,&app,1).unwrap().unwrap();
            let Load::Start(ticket,mut cache)=service.begin(1,&info.identity,info.revision,now.timestamp()) else{panic!("fresh source must start")};
            cache.fill(&wire,vec![request.clone()],now.timestamp()).await;assert!(cache.bytes()>0);ticket.finish(cache,true,now.timestamp());
            assert!(matches!(service.begin(1,&info.identity,info.revision,now.timestamp()),Load::Ready(..)),"R4 unchanged handle identity must reuse its ready cache");
            let replacement=if column=="passphrase"{"paper"}else{"b"};
            c.execute(&format!("UPDATE api_keys SET {column}=?1 WHERE id=1"),[app.cipher.encrypt(replacement)]).unwrap();
            let info=read_info(&c,&app,1).unwrap().unwrap();
            let Load::Start(ticket,cache)=service.begin(1,&info.identity,info.revision,now.timestamp()) else{panic!("new digest must start")};
            assert_eq!(cache.bytes(),0,"R must discard A market entries when encrypted {column} changes");drop(ticket);
        }
    }
    #[test]
    fn r_figure_auth_and_origin_are_captured_from_one_database_snapshot(){
        use rusqlite::hooks::{AuthAction,Authorization};
        use crate::{engine::FixedClock,web::Config};
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        let dir=tempfile::tempdir().unwrap();let file=dir.path().join("primary.sqlite3");
        let c=Connection::open(&file).unwrap();c.execute_batch(include_str!("../../../tests/fixtures/settings_primary_schema.sql")).unwrap();c.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        c.execute("INSERT INTO users(email,encrypted_password,created_at,updated_at)VALUES('r@example.com','x','2026-01-01','2026-01-01')",[]).unwrap();
        c.execute("INSERT INTO exchanges(type,name,created_at,updated_at)VALUES('Exchanges::Alpaca','Alpaca','2026-01-01','2026-01-01')",[]).unwrap();
        let now="2026-09-10T12:00:00Z".parse().unwrap();let env=|name:&str|(name=="SECRET_KEY_BASE").then(||"settings-capture-test-secret".to_string());
        let app=App::new(Config::from_env(&env).unwrap(),&env,Connection::open(&file).unwrap(),Arc::new(FixedClock(now))).unwrap();
        c.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,passphrase,created_at,updated_at)VALUES(1,1,0,1,?1,?2,?3,'2026-01-01','2026-01-01')",(app.cipher.encrypt("a"),app.cipher.encrypt("a-secret"),app.cipher.encrypt("paper"))).unwrap();
        let before=read_info(&c,&app,1).unwrap().unwrap().identity;
        let writer=Connection::open(&file).unwrap();let b=app.cipher.encrypt("b");let b_secret=app.cipher.encrypt("b-secret");let fired=Arc::new(AtomicBool::new(false));let signal=fired.clone();
        c.authorizer(Some(move|context:rusqlite::hooks::AuthContext<'_>|{
            if matches!(context.action,AuthAction::Read{table_name:"api_keys",column_name:"access_token"})&&!signal.swap(true,Ordering::SeqCst){
                writer.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=1",(&b,&b_secret)).unwrap();
            }
            Authorization::Allow
        }));
        let captured=read_info(&c,&app,1).unwrap().unwrap();assert!(fired.load(Ordering::SeqCst),"an encrypted replacement must actually commit between the auth and origin reads");
        assert_eq!(captured.credentials.key,"a","the auth read precedes the committed replacement");
        assert!(captured.identity==before,"A credentials must never be tagged with B cache provenance");
        assert_eq!(read_info(&c,&app,1).unwrap().unwrap().credentials.key,"b","the next read sees the replacement");
    }

}

#[cfg(test)]
mod tests {
    /// The fingerprint runs on every page that shows figures. At the sizes of a real install (40,000 assets, four venues'
    /// listings, two years of orders, a ledger with splits), the assets and the listings of venues the account never
    /// traded on add no work to it: no scan of either table, by a category or anything else. Counted in SQLite VM
    /// steps, which do not depend on the machine.
    #[test]
    fn the_fingerprint_reads_no_whole_table_at_real_sizes() {
        use std::sync::{Arc,atomic::{AtomicU64,Ordering}};
        let c=rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../../tests/fixtures/settings_primary_schema.sql")).unwrap();
        let t="'2024-01-01'";
        c.execute_batch(&format!("
            INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES (1,'o@example.com','x',{t},{t});
            INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (1,'Exchanges::Alpaca','Alpaca',{t},{t}),(2,'Exchanges::Kraken','Kraken',{t},{t}),
                (3,'Exchanges::Binance','Binance',{t},{t}),(4,'Exchanges::Coinbase','Coinbase',{t},{t});
            INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (1,'Fiat','usd','USD','US Dollar',{t},{t});
            -- The bots' venue at its real size: 7,967 stock listings in USD.
            WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i+1 FROM n WHERE i<7968)
            INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) SELECT i,'Stock','s'||i,'S'||i,'S'||i||' Inc',{t},{t} FROM n;
            INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at)
                SELECT id,symbol,id,9,1,0,1,2,'USD',1,2,symbol,{t},{t} FROM assets WHERE id>1;
            -- A basket of five and an index of 65, weekly and daily over two years: 1,991 orders.
            INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
                (1,1,1,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":1,\"allocations\":{{\"2\":0.2,\"3\":0.2,\"4\":0.2,\"5\":0.2,\"6\":0.2}}}}','{{}}',{t},{t}),
                (2,1,1,'Bots::DcaIndex',1,'{{\"quote_asset_id\":1,\"num_coins\":65}}','{{}}',{t},{t});
            WITH RECURSIVE n(i) AS (SELECT 10 UNION ALL SELECT i+1 FROM n WHERE i<74)
            INSERT INTO bot_index_assets(bot_id,asset_id,ticker_id,in_index,created_at,updated_at) SELECT 2,i,i,1,{t},{t} FROM n;
            WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<1991)
            INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,created_at,updated_at)
                SELECT CASE WHEN i%3=0 THEN 1 ELSE 2 END,1,'S'||(10+i%65),10+i%65,'USD',1,0.1,0.1,100,10,0,0,2,'o'||i,datetime('2024-01-01','+'||(i/3)||' days'),{t} FROM n;
            -- A ledger of 5,792 rows: a row per order, splits on the bots' venue, deposits.
            INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,quote_amount,quote_currency,raw_data,transacted_at,created_at,updated_at)
                SELECT 1,1,0,base,base_asset_id,amount,-10,'USD','{{}}',created_at,{t},{t} FROM transactions;
            INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,raw_data,transacted_at,created_at,updated_at) VALUES
                (1,1,15,'S10',10,0,'{{\"corporate_action\":\"split\",\"split_ratio\":\"2:1\"}}','2024-06-01',{t},{t}),
                (1,1,15,'S11',NULL,0,'{{\"corporate_action\":\"split\",\"split_ratio\":\"3:1\"}}','2024-09-01',{t},{t});
            WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<3799)
            INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,raw_data,transacted_at,created_at,updated_at)
                SELECT 1,1,4,'USD',1,100,'{{}}',datetime('2024-01-01','+'||(i%700)||' days'),{t},{t} FROM n;
            INSERT INTO account_balances(user_id,exchange_id,asset_id,free,synced_at,created_at,updated_at) SELECT DISTINCT 1,1,base_asset_id,1,{t},{t},{t} FROM transactions;
        ")).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM account_transactions",[],|r|r.get::<_,i64>(0)).unwrap(),5792);
        let steps=Arc::new(AtomicU64::new(0));
        let counter=steps.clone();
        c.progress_handler(100,Some(move||{counter.fetch_add(100,Ordering::Relaxed);false}));
        let measure=|c:&rusqlite::Connection|{ steps.store(0,Ordering::Relaxed); let (revision,_)=super::revision(c,1).unwrap(); (revision,steps.load(Ordering::Relaxed)) };
        let (before,alone)=measure(&c);
        // The rest of the install: 39,818 assets in all (150 of them currencies), and the other venues' listings.
        c.execute_batch(&format!("
            WITH RECURSIVE n(i) AS (SELECT 7969 UNION ALL SELECT i+1 FROM n WHERE i<39818)
            INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at)
                SELECT i,CASE WHEN i>39668 THEN 'Currency' ELSE 'Cryptocurrency' END,'c'||i,'C'||i,'C'||i,{t},{t} FROM n;
            INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at)
                SELECT id,symbol,id,9,CASE WHEN id<11145 THEN 2 WHEN id<12846 THEN 3 ELSE 4 END,0,1,2,'USD',1,2,symbol||'/USD',{t},{t} FROM assets WHERE id BETWEEN 7969 AND 14535;
        ")).unwrap();
        let (after,crowded)=measure(&c);
        assert_eq!(before,after,"nothing the figures read changed");
        assert!(crowded<=alone+alone/50,"the fingerprint took {crowded} VM steps beside the rest of the install, {alone} without it");
        // The index rounds its chart by the coarsest quote precision of every tradable listing of its venue in its quote
        // (chart::quote_decimals), held or not; another venue's listings it never reads.
        c.execute("UPDATE tickers SET quote_decimals=0 WHERE id=9000",[]).unwrap();
        assert_eq!(measure(&c).0,after,"a listing on a venue the account never used");
        c.execute("UPDATE tickers SET quote_decimals=0 WHERE id=5000",[]).unwrap();
        let precision=measure(&c).0;
        assert_ne!(precision,after,"a never-held tradable listing in the index's venue and quote");
        // Which listings are tradable is what the index asks the market for (loading::symbols): one renamed in place, of
        // the same id and precision, changes the request.
        c.execute("UPDATE tickers SET ticker='RENAMED' WHERE id=5000",[]).unwrap();
        assert_ne!(measure(&c).0,precision,"a never-held tradable listing in the index's venue and quote renamed");
    }
    #[test]
    fn stored_text_that_looks_like_a_placeholder_gets_no_token() {
        let kept=serde_json::json!({"bots":{"1":{"metrics":"<img src=\"https://images.example/figuresFormTokenPlaceholder7f3a9c.png\">"}}});
        assert_eq!(super::with_token(&kept,&vec![],"secret"),kept);
    }
    #[test]
    fn kept_figures_take_each_request_s_own_token_in_each_form_only() {
        // Two forms; stored text around them that holds a marker-like string, a token field and the token itself.
        let field="<input type=\"hidden\" name=\"authenticity_token\" value=\"a+b&amp;c&quot;\" autocomplete=\"off\" />";
        let metrics="<img src=\"figuresFormTokenPlaceholder7f3a9c\" alt=\"x\"><form><button></button></form><td>value=\"\" name=\"authenticity_token\"</td><form><button></button></form>";
        let first=metrics.find("</form>").unwrap();
        let second=metrics.rfind("</form>").unwrap();
        let kept=serde_json::json!({"bots":{"1":{"metrics":metrics,"tile":"<div>value=\"\"</div>"},"2":{"metrics":metrics}}});
        let slots=vec![("1".to_string(),vec![first,second])];
        let out=super::with_token(&kept,&slots,"a+b&c\"");
        assert_eq!(out["bots"]["1"]["metrics"],metrics.replace("</form>",&format!("{field}</form>")));
        assert_eq!(out["bots"]["1"]["tile"],kept["bots"]["1"]["tile"]);
        assert_eq!(out["bots"]["2"],kept["bots"]["2"],"a bot with no slots is as rendered");
        assert_eq!(super::with_token(&kept,&slots,""),kept,"a broadcast takes the kept fragment as it is");
    }
    #[test]
    fn many_resolved_names_stay_one_compound_term() {
        let c=rusqlite::Connection::open_in_memory().unwrap();
        let sql=format!("SELECT count(*) FROM (SELECT 0{})",super::ids_source(1..=2000));
        let n:i64=c.query_row(&sql,[],|r|r.get(0)).unwrap();
        assert_eq!(n,2001);
        assert_eq!(super::ids_source(std::iter::empty()),"");
    }
}
