//! Page-facing cache of the tracker walk: every scope's Summary the page reads (Tracker::Ledger::Summary).
//! One Rust-owned app_configs row per user; no request computes, writes or enqueues.
use super::{backfill, labels::Labels, resolver::Side, walk::{BotSale, Position, RoundTrip, Summary, Walked}};
use crate::figures::at::At;
use crate::{app_config, figures::{dec::Dec, FiguresError}};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;

pub const UNAVAILABLE: &str = "Tracker ledger unavailable: calculation failed";
const TTL_DAYS: i64 = 30;
/// The calculation a stored summary was made by: a summary of another one is cold (4: a broker name that is both a
/// security and a coin is priced by the asset its rows recorded; 5: coin-quoted rows are two legs, transfers wait for their
/// funds, a swap-in's fee joins its lots once; 6: an Alpaca CFEE is valued at its fills' price, D-TAX-CFEE-PREVIOUS-DAY;
/// 7: each opening's dollars, `opening_values`, which the history gap step reads, D-GAP-OPENINGS; 8: a security's corporate
/// action never restates a coin's lots under the symbol they share, #1061; 9: each scope's summary is an account's, keyed by
/// account id, #710 slice 5; 10: a timed Alpaca CFEE stands after its one fill at Alpaca's price, and a fill quoted at
/// nothing prices no fee, D-TAX-CFEE-MATCHED-FILL; 11: legs are owner-wide labels by coin, coins priced by identity,
/// D-TAX-COIN-IDENTITY, D-TRACKER-COIN-PRICE-KEYS).
const SCHEMA: u64 = 11;
const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
/// The bot sales' calculation (2: a timed Alpaca CFEE takes its fill's place, so a sale's basis moves, D-TAX-CFEE-MATCHED-FILL;
/// 3: lots are keyed by label, D-TAX-COIN-IDENTITY).
const SALES_SCHEMA: u64 = 3;
const MAX_SALES: usize = 1024 * 1024;
const MAX_COMPUTED_DECIMAL: usize = 1024 * 1024;
/// Publication refusals: the walk computed, but its entry is too large to store. The previous entry is kept.
pub const DECIMAL_TOO_LARGE: &str = "ledger cache computed decimal exceeds 1048576-byte limit";
pub const PAYLOAD_TOO_LARGE: &str = "ledger cache exceeds its size limit";
/// Whether a run's error is a publication refusal (nothing to state as a failed calculation).
pub fn refused(error:&str)->bool { error==DECIMAL_TOO_LARGE || error==PAYLOAD_TOO_LARGE }
#[derive(Clone, Debug)]
pub enum State { Cold, Failed(&'static str), Warm(Box<Summary>) }
#[derive(Clone, Debug)]
pub struct Version { history: String, prices: i64, producers:Value }
impl PartialEq for Version {
    fn eq(&self,other:&Self)->bool {self.history==other.history && self.prices==other.prices && crate::engine::model::stamp_set_current_for(&self.producers,&other.producers)}
}
pub fn version(c:&Connection,owner:i64)->Result<Version,FiguresError> {
    let origin=crate::sync::cache::capture_read(c,owner,&crate::tracker::Scope::All).map_err(|_|FiguresError::Data("ledger cache provenance unavailable".into()))?;
    Ok(Version { history:backfill::history_version(c,owner)?, prices:backfill::generation(c)?,producers:origin.producer_stamps() })
}
pub fn key(owner:i64)->String { format!("rust_tracker_ledger.{owner}") }
pub fn sales_key(owner:i64)->String { format!("rust_tracker_bot_sales.{owner}") }
fn encode(s:&Summary)->Value {
    let at=|a:&Option<At>|a.map(|a|a.0);
    let pairs=|l:&[(String,Dec)]|l.iter().map(|(k,v)|json!([k,v.to_s_f()])).collect::<Vec<_>>();
    json!({"positions":s.positions.iter().map(|p|json!({"symbol":p.symbol,"quantity":p.quantity.to_s_f(),"cost":p.cost.to_s_f(),"avg_cost":p.avg_cost.to_s_f(),"opened_at":at(&p.opened_at),"estimated":p.estimated,"unpriced":p.unpriced.to_s_f()})).collect::<Vec<_>>(),
        "round_trips":s.round_trips.iter().map(|t|json!({"symbol":t.symbol,"opened_at":at(&t.opened_at),"closed_at":t.closed_at.0,"quantity":t.quantity.to_s_f(),"invested":t.invested.to_s_f(),"proceeds":t.proceeds.to_s_f(),"fees":t.fees.to_s_f(),"realised":t.realised.to_s_f(),"incomplete":t.incomplete})).collect::<Vec<_>>(),
        "total_invested":s.total_invested.to_s_f(),"received":s.received.to_s_f(),"realised":s.realised.to_s_f(),"fees":s.fees.to_s_f(),"cash_usd":s.cash_usd.to_s_f(),"unpriced_proceeds":s.unpriced_proceeds.to_s_f(),
        "cash":pairs(&s.cash),"cash_basis":pairs(&s.cash_basis),"openings":pairs(&s.openings),"opening_values":pairs(&s.opening_values),"incomplete":s.incomplete,"loss_sales":s.loss_sales.iter().map(|(k,v)|json!([k,v.to_string()])).collect::<Vec<_>>(),"labels":encode_labels(&s.labels)})
}
fn side_code(s:Side)->&'static str { match s { Side::Base=>"b", Side::Quote=>"q", Side::Fee=>"f" } }
fn encode_labels(l:&Labels)->Value {
    json!({"assets":l.assets.iter().map(|(a,id,t)|json!([a,id,t])).collect::<Vec<_>>(),"legs":l.legs.iter().map(|(r,s,t)|json!([r,side_code(*s),t])).collect::<Vec<_>>(),
           "spellings":l.spellings.iter().map(|(t,r)|json!([t,r])).collect::<Vec<_>>(),"void":l.void.iter().map(|(a,b)|json!([a,b])).collect::<Vec<_>>(),"coins":l.coins.iter().map(|(t,e)|json!([t,e])).collect::<Vec<_>>()})
}
/// A summary cached before the labels were kept reads none (each balance and pending move keeps its symbol).
fn decode_labels(v:&Value)->Result<Labels,FiguresError> {
    if v.is_null() { return Ok(Labels::default()); }
    let int=|v:&Value|v.as_i64().ok_or_else(malformed);
    let side=|v:&Value|match text(v)? { "b"=>Ok(Side::Base), "q"=>Ok(Side::Quote), "f"=>Ok(Side::Fee), _=>Err(malformed()) };
    Ok(Labels { assets:array(&v["assets"])?.iter().map(|p|Ok((int(&p[0])?,int(&p[1])?,text(&p[2])?.to_string()))).collect::<Result<_,FiguresError>>()?,
                legs:array(&v["legs"])?.iter().map(|p|Ok((int(&p[0])?,side(&p[1])?,text(&p[2])?.to_string()))).collect::<Result<_,FiguresError>>()?,
                spellings:array(&v["spellings"])?.iter().map(|p|Ok((text(&p[0])?.to_string(),text(&p[1])?.to_string()))).collect::<Result<_,FiguresError>>()?,
                void:array(&v["void"])?.iter().map(|p|Ok((int(&p[0])?,int(&p[1])?))).collect::<Result<_,FiguresError>>()?,
                coins:array(&v["coins"])?.iter().map(|p|Ok((text(&p[0])?.to_string(),text(&p[1])?.to_string()))).collect::<Result<_,FiguresError>>()?, identity:String::new() })
}
/// Every computed decimal a summary stores: the bound a reader applies is applied before publishing too.
fn decimals(s:&Summary)->impl Iterator<Item=&Dec> {
    [&s.total_invested,&s.received,&s.realised,&s.fees,&s.cash_usd,&s.unpriced_proceeds].into_iter()
        .chain(s.cash.iter().chain(&s.cash_basis).chain(&s.openings).chain(&s.opening_values).map(|(_,v)|v))
        .chain(s.positions.iter().flat_map(|p|[&p.quantity,&p.cost,&p.avg_cost,&p.unpriced]))
        .chain(s.round_trips.iter().flat_map(|t|[&t.quantity,&t.invested,&t.proceeds,&t.fees,&t.realised]))
}
fn malformed()->FiguresError { FiguresError::Data("ledger cache payload is unreadable".into()) }
fn text(v:&Value)->Result<&str,FiguresError> { v.as_str().ok_or_else(malformed) }
fn array(v:&Value)->Result<&Vec<Value>,FiguresError> { v.as_array().ok_or_else(malformed) }
fn decimal(v:&Value)->Result<Dec,FiguresError> {
    let text=text(v)?;
    if text.len()>MAX_COMPUTED_DECIMAL {
        crate::engine::log("[tracker] ledger cache cold: computed decimal exceeds 1048576-byte limit");
        return Err(FiguresError::NotComputed("ledger cache computed decimal exceeds 1048576-byte limit".into()));
    }
    // Our encoder writes fixed-point strings, never exponents. Bound the expanded
    // size before calling the existing computed-number parser; keep strict for inputs.
    let unsigned=text.strip_prefix('-').unwrap_or(text);
    let Some((whole,fraction))=unsigned.split_once('.') else {return Err(malformed())};
    let digits=|s:&str| !s.is_empty() && s.bytes().all(|b|b.is_ascii_digit());
    if !digits(whole) || !digits(fraction) {return Err(malformed())}
    Ok(Dec::parse(text)?)
}
fn boolean(v:&Value)->Result<bool,FiguresError> { v.as_bool().ok_or_else(malformed) }
fn instant(v:&Value)->Result<At,FiguresError> { v.as_i64().map(At).ok_or_else(malformed) }
fn optional_instant(v:&Value)->Result<Option<At>,FiguresError> { if v.is_null() {Ok(None)} else {instant(v).map(Some)} }
fn decode(v:&Value)->Result<Summary,FiguresError> {
    let pairs=|v:&Value|array(v)?.iter().map(|p|Ok((text(&p[0])?.into(),decimal(&p[1])?))).collect::<Result<Vec<_>,FiguresError>>();
    Ok(Summary { positions:array(&v["positions"] )?.iter().map(|p|Ok(Position { symbol:text(&p["symbol"] )?.into(),quantity:decimal(&p["quantity"] )?,cost:decimal(&p["cost"] )?,avg_cost:decimal(&p["avg_cost"] )?,opened_at:optional_instant(&p["opened_at"])?,estimated:boolean(&p["estimated"] )?,unpriced:decimal(&p["unpriced"] )? })).collect::<Result<_,FiguresError>>()?,
        round_trips:array(&v["round_trips"])?.iter().map(|t|Ok(RoundTrip { symbol:text(&t["symbol"])?.into(),opened_at:optional_instant(&t["opened_at"])?,closed_at:instant(&t["closed_at"])?,quantity:decimal(&t["quantity"])?,invested:decimal(&t["invested"])?,proceeds:decimal(&t["proceeds"])?,fees:decimal(&t["fees"])?,realised:decimal(&t["realised"])?,incomplete:boolean(&t["incomplete"])? })).collect::<Result<_,FiguresError>>()?,
        total_invested:decimal(&v["total_invested"] )?,received:decimal(&v["received"])?,realised:decimal(&v["realised"])?,fees:decimal(&v["fees"])?,cash_usd:decimal(&v["cash_usd"])?,unpriced_proceeds:decimal(&v["unpriced_proceeds"])?,
        cash:pairs(&v["cash"] )?,cash_basis:pairs(&v["cash_basis"] )?,openings:pairs(&v["openings"])?,opening_values:pairs(&v["opening_values"])?,incomplete:boolean(&v["incomplete"] )?,loss_sales:array(&v["loss_sales"] )?.iter().map(|p|Ok((text(&p[0])?.into(),NaiveDate::parse_from_str(text(&p[1])?,"%Y-%m-%d").map_err(|_|malformed())?))).collect::<Result<_,FiguresError>>()?,labels:decode_labels(&v["labels"])? })
}
/// Called within the producer's write transaction; changed inputs cannot publish as current.
pub fn publish(c:&crate::engine::model::FencedTransaction<'_>,owner:i64,before:&Version,walked:Option<&Walked>,now:DateTime<Utc>)->Result<bool,FiguresError> {
    if &version(c,owner)? != before { return Ok(false); }
    let origin=crate::sync::cache::capture_read(c,owner,&crate::tracker::Scope::All).map_err(|_|FiguresError::Data("ledger cache provenance unavailable".into()))?;
    if !origin.ledger_is_current(c).map_err(|_|FiguresError::Data("ledger cache producer unavailable".into()))? {return Ok(false)}
    // The read bound applies at publish too: an entry the reader would refuse is never written.
    for summary in walked.into_iter().flat_map(|w|std::iter::once(&w.whole).chain(w.venues.iter().map(|(_,s)|s))) {
        for decimal in decimals(summary) {
            if decimal.to_s_f().len()>MAX_COMPUTED_DECIMAL {
                crate::engine::log("[tracker] ledger cache publish refused: computed decimal exceeds 1048576-byte limit");
                return Err(FiguresError::NotComputed(DECIMAL_TOO_LARGE.into()));
            }
        }
    }
    // Schema 3: the page's figures too (round trips, received, realised, fees, cash at today's rates, unpriced proceeds,
    // openings, position dates). Schema 1 and 2 entries read cold and the next pass republishes.
    let expires=(now+Duration::days(TTL_DAYS)).to_rfc3339();
    let payload=json!({"schema":SCHEMA,"owner":owner,"history":before.history,"prices":before.prices,"producers":before.producers,"expires":expires,"whole":walked.map(|w|encode(&w.whole)),"venues":walked.map(|w|w.venues.iter().map(|(id,s)|json!([id,encode(s)])).collect::<Vec<_>>())}).to_string();
    if payload.len()>MAX_PAYLOAD {crate::engine::log("[tracker] ledger cache publish refused: payload exceeds 8388608-byte limit");return Err(FiguresError::NotComputed(PAYLOAD_TOO_LARGE.into()));}
    app_config::set_plain(c,&key(owner),&payload,now).map_err(FiguresError::Data)?;
    // The bot sales beside it, same fence, same inputs: what the figures read for a sell-only bot's P/L. A failed walk or an
    // oversized set publishes none (nobody's P/L shows); the ledger entry above is never refused for it.
    let sales_row=|sales:Vec<Value>|json!({"schema":SALES_SCHEMA,"owner":owner,"history":before.history,"prices":before.prices,"producers":before.producers,"expires":expires,"sales":sales}).to_string();
    if walked.is_none() {crate::engine::log("[tracker] bot sales published empty: the walk failed");}
    let mut sales=sales_row(walked.map(|w|w.bot_sales.iter().map(|b|json!([b.order,b.at.0,b.amount.to_s_f(),b.quote,b.cost.to_s_f(),b.known])).collect()).unwrap_or_default());
    if sales.len()>MAX_SALES {crate::engine::log("[tracker] bot sales publish refused: payload exceeds 1048576-byte limit");sales=sales_row(vec![])}
    app_config::set_plain(c,&sales_key(owner),&sales,now).map_err(FiguresError::Data)?;
    Ok(true)
}
/// The published entry's own `history`, `prices`, `expires` and `producers`, as stored. `expires` is new with every
/// publish, so equal fingerprints are one publication (tracker::gaps keeps its row against it).
#[derive(Clone, Debug, PartialEq)]
pub struct Fingerprint(Value);
impl Fingerprint {
    fn of(v:&Value)->Self { Fingerprint(json!({"history":v["history"],"prices":v["prices"],"expires":v["expires"],"producers":v["producers"]})) }
    pub fn to_json(&self)->&Value { &self.0 }
    pub fn from_json(v:&Value)->Self { Self::of(v) }
}
/// Every scope of a warm entry, with the publication it came from.
#[derive(Clone, Debug)]
pub enum Published { Cold, Failed(&'static str), Warm { whole:Box<Summary>, venues:Vec<(i64,Summary)>, from:Fingerprint } }
impl Published {
    /// The scope a page reads: the whole portfolio, or one account's (empty when it has none).
    pub fn scoped(self,scope:&super::Scope)->State {
        self.account(match scope {super::Scope::All=>None,super::Scope::Account(a)=>Some(a.id),super::Scope::EmptyLegacyExchange(_)=>Some(0)})
    }
    /// `scoped` by account id (None: the whole portfolio).
    pub fn account(self,account:Option<i64>)->State {
        match self {
            Published::Cold=>State::Cold,
            Published::Failed(m)=>State::Failed(m),
            Published::Warm{whole,venues,..}=>State::Warm(match account {
                None=>whole,
                Some(id)=>Box::new(venues.into_iter().find(|(venue,_)|*venue==id).map_or_else(Summary::empty,|(_,summary)|summary)),
            }),
        }
    }
}
/// The stored entry's fingerprint, unvalidated (None without a readable entry): what a write guarded on a publication
/// re-reads under its write lock.
pub fn fingerprint(c:&Connection,owner:i64)->Result<Option<Fingerprint>,FiguresError> {
    let Some(raw)=app_config::get_plain(c,&key(owner)).map_err(FiguresError::Data)? else {return Ok(None)};
    Ok(serde_json::from_str::<Value>(&raw).ok().filter(Value::is_object).map(|v|Fingerprint::of(&v)))
}
/// Cold includes absent/expired/obsolete/malformed cache entries, as 2.x's shape guard does.
/// SQL failures propagate; a computed failure has its own safe, stated reason.
/// `account`: an account id (None: the whole portfolio).
pub fn read(c:&Connection,owner:i64,account:Option<i64>,now:DateTime<Utc>)->Result<State,FiguresError> {
    Ok(read_published(c,owner,now)?.account(account))
}
/// The entry stored under `key`, parsed, when it is one the current inputs made: schema, owner, expiry, `version()`, producers
/// and `ledger_is_current` all hold. None is Cold. The ledger entry and the bot sales row are both read through it.
fn validated(c:&Connection,key:&str,schema:u64,owner:i64,now:DateTime<Utc>,limit:usize)->Result<Option<Value>,FiguresError> {
    let Some(raw)=app_config::get_plain(c,key).map_err(FiguresError::Data)? else {return Ok(None)};
    if raw.len()>limit {crate::engine::log(&format!("[tracker] cache cold: {key} exceeds {limit}-byte limit"));return Ok(None)}
    let Ok(v)=serde_json::from_str::<Value>(&raw) else {return Ok(None)};
    if v["schema"].as_u64()!=Some(schema) || v["owner"].as_i64()!=Some(owner) {return Ok(None)}
    let Some(expires)=v["expires"].as_str().and_then(|s|s.parse::<DateTime<Utc>>().ok()) else {return Ok(None)};
    if now>=expires {return Ok(None)}
    let current=version(c,owner)?;
    if v["history"].as_str()!=Some(current.history.as_str()) || v["prices"].as_i64()!=Some(current.prices) {return Ok(None)}
    if !crate::engine::model::stamp_set_current_for(&v["producers"],&current.producers) {return Ok(None)}
    let origin=crate::sync::cache::capture_read(c,owner,&crate::tracker::Scope::All).map_err(|_|FiguresError::Data("ledger cache provenance unavailable".into()))?;
    if !origin.ledger_is_current(c).map_err(|_|FiguresError::Data("ledger cache producer unavailable".into()))? {return Ok(None)}
    Ok(Some(v))
}
/// `read` before the scope is picked: one parse, every scope, and the publication's fingerprint.
pub fn read_published(c:&Connection,owner:i64,now:DateTime<Utc>)->Result<Published,FiguresError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=read_published(&tx,owner,now)?;tx.commit()?;return Ok(out)}
    let Some(v)=validated(c,&key(owner),SCHEMA,owner,now,MAX_PAYLOAD)? else {return Ok(Published::Cold)};
    if !v.as_object().is_some_and(|o|o.contains_key("whole") && o.contains_key("venues")) {return Ok(Published::Cold)}
    if v["whole"].is_null() && v["venues"].is_null() {return Ok(Published::Failed(UNAVAILABLE))}
    type Scopes=(Summary,Vec<(i64,Summary)>);
    let decoded=(||->Result<Scopes,FiguresError>{
        let whole=decode(&v["whole"])?;
        let mut venues=vec![];
        for pair in array(&v["venues"])? {
            let pair=array(pair)?;
            if pair.len()!=2 {return Err(malformed())}
            venues.push((pair[0].as_i64().ok_or_else(malformed)?,decode(&pair[1])?));
        }
        Ok((whole,venues))
    })();
    match decoded {
        Ok((whole,venues))=>Ok(Published::Warm{whole:Box::new(whole),venues,from:Fingerprint::of(&v)}),
        Err(_)=>Ok(Published::Cold),
    }
}

/// The bot sales of the walk the ledger entry was made from. Cold unless validated exactly as the ledger entry is.
#[derive(Clone, Debug)]
pub enum Sales { Cold, Warm { by_order: HashMap<i64, BotSale>, expires: DateTime<Utc> } }
impl Sales {
    /// A hash of the sales (0 when Cold, never 0 when Warm): what figures rendered from them are held to. Not of the
    /// expiry: a republish of the same sales keeps kept figures shown; expiry is the read's (`bot_sales`) and the fill's
    /// (`until` capped at `expires`).
    pub fn stamp(&self)->u64 {
        use std::hash::{Hash,Hasher};
        let Sales::Warm{by_order,..}=self else {return 0};
        let mut rows:Vec<_>=by_order.values().map(|b|(b.order,b.at.0,b.amount.to_s_f(),b.quote.clone(),b.cost.to_s_f(),b.known)).collect();
        rows.sort_by_key(|r|r.0);
        let mut h=std::collections::hash_map::DefaultHasher::new();
        rows.hash(&mut h);
        h.finish().max(1)
    }
}
pub fn bot_sales(c:&Connection,owner:i64,now:DateTime<Utc>)->Result<Sales,FiguresError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=bot_sales(&tx,owner,now)?;tx.commit()?;return Ok(out)}
    let Some(v)=validated(c,&sales_key(owner),SALES_SCHEMA,owner,now,MAX_SALES)? else {return Ok(Sales::Cold)};
    let Some(expires)=v["expires"].as_str().and_then(|s|s.parse::<DateTime<Utc>>().ok()) else {return Ok(Sales::Cold)};
    // Published beside the ledger entry with the same expiry: an invalidation that deletes that entry (a rewrite `history`
    // may not see), or a ledger entry of another publish, leaves these sales cold too.
    let ledger=app_config::get_plain(c,&key(owner)).map_err(FiguresError::Data)?;
    let ledger_expires=ledger.as_deref().and_then(|raw|serde_json::from_str::<Value>(raw).ok()).and_then(|l|l["expires"].as_str().map(str::to_string));
    if ledger_expires.as_deref()!=v["expires"].as_str() {return Ok(Sales::Cold)}
    let decoded=(||->Result<HashMap<i64,BotSale>,FiguresError>{
        let mut by_order=HashMap::new();
        for row in array(&v["sales"])? {
            let row=array(row)?;
            if row.len()!=6 {return Err(malformed())}
            let order=row[0].as_i64().ok_or_else(malformed)?;
            by_order.insert(order,BotSale{order,at:instant(&row[1])?,amount:decimal(&row[2])?,quote:text(&row[3])?.into(),cost:decimal(&row[4])?,known:boolean(&row[5])?});
        }
        Ok(by_order)
    })();
    Ok(decoded.map_or(Sales::Cold,|by_order|Sales::Warm{by_order,expires}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn publish(c:&Connection,owner:i64,before:&Version,walked:Option<&Walked>,venue:i64,now:DateTime<Utc>)->Result<bool,FiguresError>{
        let tx=c.unchecked_transaction()?;let fenced=crate::engine::model::fence_versions(&tx,&[]).map_err(|_|malformed())?;
        let _=venue;let out=super::publish(&fenced,owner,before,walked,now)?;tx.commit()?;Ok(out)
    }
    fn db()->Connection {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys(id INTEGER,user_id INTEGER,exchange_id INTEGER,key_type INTEGER,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT); CREATE TABLE app_configs(key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT); CREATE TABLE account_transactions(id INTEGER,user_id INTEGER,updated_at TEXT); CREATE TABLE historical_prices(id INTEGER); CREATE TABLE exchange_account_balances(id INTEGER,usd_value NUMERIC);").unwrap(); c.execute_batch(crate::identity::TEST_ACCOUNT_TABLES).unwrap(); c
    }
    fn now()->DateTime<Utc> {"2026-10-08T12:00:00Z".parse().unwrap()}
    fn walked()->Walked {
        let mut whole=Summary::empty(); whole.total_invested=Dec::strict("123.4567890123456789").unwrap();
        whole.positions.push(Position{symbol:"AAPL".into(),quantity:Dec::strict("2.5").unwrap(),cost:Dec::strict("123.4567890123456789").unwrap(),avg_cost:Dec::strict("49.38271560493827156").unwrap(),opened_at:Some(At(1_790_000_000_123_456_789)),estimated:true,unpriced:Dec::zero()});
        whole.round_trips.push(RoundTrip{symbol:"MSFT".into(),opened_at:None,closed_at:At(1_790_000_100_000_000_000),quantity:Dec::strict("1.5").unwrap(),invested:Dec::strict("300.01").unwrap(),proceeds:Dec::strict("280.5").unwrap(),fees:Dec::strict("0.25").unwrap(),realised:Dec::strict("-19.76").unwrap(),incomplete:true});
        whole.received=Dec::strict("3.3").unwrap();whole.realised=Dec::strict("-19.76").unwrap();whole.fees=Dec::strict("0.25").unwrap();whole.cash_usd=Dec::strict("7.125").unwrap();whole.unpriced_proceeds=Dec::strict("0.5").unwrap();
        whole.openings=vec![("BTC".into(),Dec::strict("0.01").unwrap())];whole.opening_values=vec![("BTC".into(),Dec::strict("600.123456789").unwrap())];
        whole.cash=vec![("USD".into(),Dec::strict("-0.0").unwrap())];whole.cash_basis=vec![("USD".into(),Dec::strict("1.01").unwrap())];whole.incomplete=true;whole.loss_sales=vec![("AAPL".into(),now().date_naive())];
        Walked{venues:vec![(7,whole.clone())],whole,terms:vec![],bot_sales:vec![]}
    }
    fn warm(c:&Connection,exchange:Option<i64>)->Summary {
        let State::Warm(s)=read(c,1,exchange,now()).unwrap() else { panic!("expected warm cache") };*s
    }
    fn sale(order:i64,cost:&str)->BotSale { BotSale{order,at:At(1_790_000_000_000_000_000),amount:Dec::strict("0.5").unwrap(),quote:"EUR".into(),cost:Dec::strict(cost).unwrap(),known:true} }
    fn with_sales(sales:Vec<BotSale>)->Walked { let mut w=walked(); w.bot_sales=sales; w }
    fn sales(c:&Connection)->Sales { bot_sales(c,1,now()).unwrap() }
    #[test]
    fn bot_sales_roundtrip_and_stamp() {
        let c=db();let v=version(&c,1).unwrap();
        assert!(matches!(sales(&c),Sales::Cold));
        publish(&c,1,&v,Some(&with_sales(vec![sale(5,"10.25"),sale(9,"0.0")])),7,now()).unwrap();
        let Sales::Warm{by_order,..}=sales(&c) else {panic!("warm")};
        assert_eq!(by_order.len(),2);assert_eq!(by_order[&5],sale(5,"10.25"));assert_eq!(by_order[&9].cost,Dec::zero());
        let a=sales(&c).stamp();
        // A republish of the same sales, a moment later (a later expiry): the same stamp, so kept figures stay shown.
        publish(&c,1,&v,Some(&with_sales(vec![sale(9,"0.0"),sale(5,"10.25")])),7,now()+Duration::minutes(5)).unwrap();
        assert_eq!(sales(&c).stamp(),a,"the stamp is the sales, not their expiry");
        publish(&c,1,&v,Some(&with_sales(vec![sale(5,"10.26")])),7,now()).unwrap();
        let b=sales(&c).stamp();
        assert!(a!=0 && b!=0 && a!=b);
        assert_eq!(Sales::Cold.stamp(),0);
        warm(&c,None);
    }
    #[test]
    fn bot_sales_cold_when_stale_expired_absent_or_malformed() {
        let c=db();let v=version(&c,1).unwrap();
        publish(&c,1,&v,Some(&with_sales(vec![sale(5,"10")])),7,now()).unwrap();
        assert!(matches!(bot_sales(&c,1,now()+Duration::days(31)).unwrap(),Sales::Cold));
        let good=app_config::get_plain(&c,&sales_key(1)).unwrap().unwrap();
        // Another owner's key holding this owner's row: refused by the owner check, not for want of a row.
        app_config::set_plain(&c,&sales_key(2),&good,now()).unwrap();
        assert!(matches!(bot_sales(&c,2,now()).unwrap(),Sales::Cold));
        // `"schema":1`: the release before D-TAX-CFEE-MATCHED-FILL's sales.
        for bad in ["not json".to_string(),good.replace("\"sales\":[[","\"sales\":[[1,"),good.replace(&format!("\"schema\":{SALES_SCHEMA}"),"\"schema\":1")] {
            app_config::set_plain(&c,&sales_key(1),&bad,now()).unwrap();assert!(matches!(sales(&c),Sales::Cold));
        }
        app_config::set_plain(&c,&sales_key(1),&good,now()).unwrap();assert!(matches!(sales(&c),Sales::Warm{..}));
        c.execute("UPDATE account_transactions SET updated_at='2026-10-09 00:00:00'",[]).unwrap();
        c.execute("INSERT INTO account_transactions(id,user_id,updated_at) VALUES(1,1,'2026-10-09 00:00:00')",[]).unwrap();
        assert!(matches!(sales(&c),Sales::Cold));
        c.execute("DELETE FROM app_configs WHERE key=?1",[sales_key(1)]).unwrap();
        assert!(matches!(sales(&c),Sales::Cold));
    }
    #[test]
    fn bot_sales_are_cold_without_the_ledger_entry_they_were_published_with() {
        let c=db();let v=version(&c,1).unwrap();
        publish(&c,1,&v,Some(&with_sales(vec![sale(5,"10")])),7,now()).unwrap();
        let ledger=app_config::get_plain(&c,&key(1)).unwrap().unwrap();
        // An invalidation that deletes the ledger entry (a rewrite `history` may not see) withdraws the sales with it.
        c.execute("DELETE FROM app_configs WHERE key=?1",[key(1)]).unwrap();
        assert!(matches!(sales(&c),Sales::Cold),"the ledger entry is gone");
        // A ledger entry of another publish (another expiry) is not the one these sales were published beside.
        let other:Value=serde_json::from_str(&ledger).unwrap();
        let mut other=other;other["expires"]=json!((now()+Duration::days(2)).to_rfc3339());
        app_config::set_plain(&c,&key(1),&other.to_string(),now()).unwrap();
        assert!(matches!(sales(&c),Sales::Cold),"another publish's ledger entry");
        app_config::set_plain(&c,&key(1),&ledger,now()).unwrap();
        assert!(matches!(sales(&c),Sales::Warm{..}));
    }
    #[test]
    fn failed_walk_or_oversized_sales_publish_none_and_the_ledger_still_publishes() {
        let c=db();let v=version(&c,1).unwrap();
        assert!(publish(&c,1,&v,None,7,now()).unwrap());
        let Sales::Warm{by_order,..}=sales(&c) else {panic!("warm")};assert!(by_order.is_empty());
        let big:Vec<BotSale>=(0..20_000).map(|i|sale(i,"123456789.123456789")).collect();
        assert!(publish(&c,1,&v,Some(&with_sales(big)),7,now()).unwrap());
        let Sales::Warm{by_order,..}=sales(&c) else {panic!("warm")};assert!(by_order.is_empty());
        warm(&c,None);
    }
    #[test]
    fn cache_publish_decimal_limit_keeps_previous_entry() {
        let c=db();let v=version(&c,1).unwrap();
        publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
        let before=app_config::get_plain(&c,&key(1)).unwrap();
        let timestamps: (String,String)=c.query_row("SELECT created_at,updated_at FROM app_configs WHERE key=?1",[key(1)],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        let changes=c.total_changes();
        // Exercise every decimal field and both scopes, below the payload limit.
        for scope in [false,true] {
            for field in 0..21 {
                let mut w=walked();
                let s=if scope {&mut w.venues[0].1}else{&mut w.whole};
                let huge=Dec::parse(&format!("0.{}1","0".repeat(MAX_COMPUTED_DECIMAL))).unwrap();
                match field {
                    0=>s.total_invested=huge,1=>s.positions[0].quantity=huge,
                    2=>s.positions[0].cost=huge,3=>s.positions[0].avg_cost=huge,
                    4=>s.positions[0].unpriced=huge,5=>s.cash[0].1=huge,
                    6=>s.cash_basis[0].1=huge,7=>s.positions[0].cost=huge.neg(),
                    8=>s.received=huge,9=>s.realised=huge,10=>s.fees=huge,11=>s.cash_usd=huge,12=>s.unpriced_proceeds=huge,13=>s.openings[0].1=huge,
                    14=>s.round_trips[0].quantity=huge,15=>s.round_trips[0].invested=huge,16=>s.round_trips[0].proceeds=huge,17=>s.round_trips[0].fees=huge,
                    18=>s.round_trips[0].realised=huge,19=>s.opening_values[0].1=huge,_=>s.realised=huge.neg(),
                }
                let result=publish(&c,1,&v,Some(&w),7,now());
                assert!(result.is_err(),"publish must refuse oversize decimal in scope {scope} field {field}");
                assert_eq!(app_config::get_plain(&c,&key(1)).unwrap(),before,"previous cache must survive rejected publication");
                assert_eq!(c.query_row("SELECT created_at,updated_at FROM app_configs WHERE key=?1",[key(1)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap(),timestamps);
                assert_eq!(c.total_changes(),changes,"rejected publication must execute no write");
                warm(&c,None);
            }
        }
    }
    #[test]
    fn cache_publish_decimal_limit_logs_reason() {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","tracker::cache::tests::cache_publish_decimal_limit_keeps_previous_entry","--nocapture"]).output().unwrap();
        assert!(output.status.success());
        let log=String::from_utf8(output.stdout).unwrap();
        assert!(log.contains("ledger cache publish refused: computed decimal exceeds 1048576-byte limit"),"publish refusal must state safe reason: {log}");
    }
    #[test]
    fn labels_roundtrip() {
        let l=Labels{assets:vec![(-2,21,"BTG (Openverse Network)".into())],legs:vec![(1,Side::Base,"BTG (Bitcoin Gold)".into()),(1,Side::Fee,"X".into())],spellings:vec![("BTG (Bitcoin Gold)".into(),"BTG".into())],void:vec![(2,3),(3,2)],coins:vec![("BTG (Bitcoin Gold)".into(),"bitcoin-gold".into())],identity:String::new()};
        assert_eq!(decode_labels(&encode_labels(&l)).unwrap(),l);
        assert_eq!(decode_labels(&Value::Null).unwrap(),Labels::default(),"a summary cached before the labels");
    }
    #[test]
    fn cache_roundtrip_is_exact_owner_scoped_and_read_only() {
        let c=db();let w=walked();let v=version(&c,1).unwrap();
        assert!(publish(&c,1,&v,Some(&w),7,now()).unwrap());
        let changes=c.total_changes();c.pragma_update(None,"query_only",true).unwrap();
        for scope in [None,Some(7)] {let s=warm(&c,scope);assert_eq!(encode(&s),encode(&w.whole));}
        assert_eq!(warm(&c,Some(8)).total_invested.to_s_f(),"0.0");
        assert!(matches!(read(&c,42,None,now()).unwrap(),State::Cold));
        assert_eq!(c.total_changes(),changes);
    }
    #[test]
    fn cache_invalidates_transactions_prices_and_expiry_but_not_balances_or_foreign_rows() {
        for sql in ["INSERT INTO account_transactions VALUES(1,1,'2026-10-08 12:00:01')","INSERT INTO historical_prices VALUES(1)"] {
            let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,Some(&walked()),7,now()).unwrap();c.execute_batch(sql).unwrap();
            assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
            assert!(!publish(&c,1,&v,Some(&walked()),7,now()).unwrap(),"must reject a mid-walk change");
        }
        let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
        c.execute_batch("INSERT INTO account_transactions VALUES(2,42,'2026-10-09'); INSERT INTO exchange_account_balances VALUES(1,100000)").unwrap();warm(&c,None);
        assert!(matches!(read(&c,1,None,now()+Duration::days(30)-Duration::microseconds(1)).unwrap(),State::Warm(_)));
        assert!(matches!(read(&c,1,None,now()+Duration::days(30)).unwrap(),State::Cold));
    }
    #[test]
    fn cache_malformed_scope_never_becomes_a_numeric_empty_summary() {
        let c=db();let v=version(&c,1).unwrap();
        for (field,value) in [("venues",serde_json::Value::Null),("venues",serde_json::json!("bad")),("venues",serde_json::json!([[]])),("venues",serde_json::json!([["7",{}]])),("venues",serde_json::json!([[7,{}]])),("whole",serde_json::json!({}))] {
            publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
            let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw[field]=value;
            app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();
            for scope in [None,Some(7),Some(8)] {assert!(matches!(read(&c,1,scope,now()).unwrap(),State::Cold),"malformed {field} in scope {scope:?}");}
        }
    }
    #[test]
    fn cache_size_limits_refuse_writes_and_make_oversized_reads_cold() {
        let c=db();let v=version(&c,1).unwrap();let mut huge=walked();huge.whole.positions[0].symbol="x".repeat(MAX_PAYLOAD);
        assert!(publish(&c,1,&v,Some(&huge),7,now()).is_err());
        assert!(app_config::get_plain(&c,&key(1)).unwrap().is_none());
        publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
        let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw["padding"]=serde_json::json!("x".repeat(MAX_PAYLOAD));
        app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();
        assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        // Schema 3's figures are bounded on read like the rest: an oversized one never becomes money.
        let huge=serde_json::json!(format!("0.{}1","0".repeat(MAX_COMPUTED_DECIMAL)));
        for path in [&["whole","received"][..],&["whole","realised"],&["whole","fees"],&["whole","cash_usd"],&["whole","unpriced_proceeds"]] {
            publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
            let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw[path[0]][path[1]]=huge.clone();
            app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();
            assert!(matches!(read(&c,1,None,now()),Ok(State::Cold)),"oversized {path:?}");
        }
        for (list,field) in [("round_trips","fees"),("round_trips","realised"),("openings","1"),("opening_values","1")] {
            publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
            let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();
            if field=="1" {raw["whole"][list][0][1]=huge.clone()} else {raw["whole"][list][0][field]=huge.clone()}
            app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();
            assert!(matches!(read(&c,1,None,now()),Ok(State::Cold)),"oversized {list}.{field}");
        }
    }
    #[test]
    fn cache_failure_malformed_schema_and_bad_decimals_never_return_money() {
        let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,None,7,now()).unwrap();
        assert!(matches!(read(&c,1,None,now()).unwrap(),State::Failed(UNAVAILABLE)));
        for raw in ["not JSON","null","{}"] {app_config::set_plain(&c,&key(1),raw,now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));}
        for (field,value) in [("schema",serde_json::json!(0)),("schema",serde_json::json!(2)),("owner",serde_json::json!(42))] {
            publish(&c,1,&v,Some(&walked()),7,now()).unwrap();let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw[field]=value;app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        }
        publish(&c,1,&v,Some(&walked()),7,now()).unwrap();let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw["whole"]["total_invested"]=serde_json::json!("NaN");app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        c.execute_batch("DROP TABLE app_configs").unwrap();assert!(read(&c,1,None,now()).is_err());
    }

    /// A summary the previous calculation stored is cold once this one runs, though no row and no price moved.
    #[test]
    fn a_summary_of_the_previous_calculation_reads_cold_with_nothing_written() {
        let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
        assert!(matches!(read(&c,1,None,now()).unwrap(),State::Warm(_)));
        let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();
        // The release before D-TAX-CFEE-MATCHED-FILL wrote schema 9.
        raw["schema"]=serde_json::json!(9);app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();
        c.pragma_update(None,"query_only",true).unwrap();let before=c.total_changes();
        assert_eq!(version(&c,1).unwrap(),v,"neither the rows nor the prices moved");
        assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        assert_eq!(c.total_changes(),before);
    }

    #[test]
    fn cache_real_split_walk_roundtrips_long_computed_decimals() {
        use crate::{figures::at::At,tracker::{fx::FxTable,prices::{NoFetch,PriceService,Reference},rows::{Kind,Stored},walk}};
        // Each adjustment on its own day: same-day splits are netted into one restatement (D-ALPACA-SPLIT-GROUPING),
        // and the 287-character decimal needs three successive restatements.
        let row=|id,kind,amount:&str,value:&str,days:i64|Stored{id,exchange_id:7,venue:"alpaca".into(),kind,base:"AAPL".into(),amount:Dec::strict(amount).unwrap(),quote:Some("USD".into()),quote_amount:Some(Dec::strict(value).unwrap()),
            fee_currency:None,fee_amount:None,tx_id:None,group:None,at:At::from_utc(now()-chrono::Duration::days(days)).unwrap(),per_share:None,stated:None,linked_to:None,to_venue:None,to_exchange_id:None,base_asset:None,cfee:Default::default(),bot_order:None,account:7,to_account:None, to_base: None};
        let mut rows=vec![row(1,Kind::Buy,"9","100",4)];
        for i in 0..3 {rows.push(row(2+i,Kind::Adjustment,"2","0",3-i));}
        let (r,fx)=(Reference::default(),FxTable::default());let mut none=NoFetch;
        let mut ps=PriceService::new(&r,&fx,now().date_naive(),&mut none);
        let w=walk::scopes(&rows,&Default::default(),&mut ps,At::from_utc(now()).unwrap()).unwrap();
        let longest=[&w.whole.positions[0].quantity,&w.whole.positions[0].cost,&w.whole.positions[0].avg_cost].into_iter().map(|d|d.to_s_f().len()).max().unwrap();
        assert!(longest==287,"three split adjustments must exercise the 287-character regression, got {longest}: {:?}",encode(&w.whole));
        let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,Some(&w),7,now()).unwrap();
        c.pragma_update(None,"query_only",true).unwrap();let before=c.total_changes();
        for (scope,expected) in [(None,&w.whole),(Some(7),w.venue(7).unwrap())] {
            let State::Warm(s)=read(&c,1,scope,now()).unwrap() else {panic!("real split walk must round-trip as Warm")};
            assert_eq!(encode(&s).to_string().as_bytes(),encode(expected).to_string().as_bytes());
        }
        assert_eq!(c.total_changes(),before);
    }
    #[test]
    fn cache_computed_decimal_limit_and_grammar_are_bounded() {
        let c=db();let v=version(&c,1).unwrap();
        // Computed cache decimals are plain to_s_f strings, up to 1 MiB each.
        for text in [format!("0.{}1","0".repeat(1024*1024-3)),"-0.0".into()] {
            let mut w=walked();w.whole.total_invested=Dec::parse(&text).unwrap();
            publish(&c,1,&v,Some(&w),7,now()).unwrap();assert_eq!(warm(&c,None).total_invested.to_s_f(),text);
        }
        for text in [format!("0.{}1","0".repeat(1024*1024)),"1e9223372036854775807".into()," 1.0 ".into(),"NaN".into()] {
            publish(&c,1,&v,Some(&walked()),7,now()).unwrap();
            let mut raw:Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw["whole"]["total_invested"]=json!(text);
            app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        }
        assert!(Dec::strict(&format!("0.{}1","1".repeat(287))).is_err(),"external decimal limits stay unchanged");
    }
    #[test]
    fn cache_computed_limit_logs_safe_reason() {
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","tracker::cache::tests::cache_computed_decimal_limit_and_grammar_are_bounded","--nocapture"])
            .output().unwrap();
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stdout));
        let log=String::from_utf8(output.stdout).unwrap();
        assert!(log.contains("ledger cache cold: computed decimal exceeds 1048576-byte limit"),"oversized decimal Cold reason must be logged: {log}");
        assert!(!log.contains("1e9223372036854775807"),"stored payload must not appear in logs");
    }
}
