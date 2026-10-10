//! Page-facing cache of the existing supported walk, not the full Rails page Summary.
//! One Rust-owned app_configs row per user; no request computes, writes or enqueues.
use super::{backfill, walk::{Position, Summary, Walked}};
use crate::{app_config, figures::{dec::Dec, FiguresError}};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};

pub const UNAVAILABLE: &str = "Tracker ledger unavailable: calculation failed";
const TTL_DAYS: i64 = 30;
const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
const MAX_COMPUTED_DECIMAL: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub enum State { Cold, Failed(&'static str), Warm(Box<Summary>) }
#[derive(Clone, Debug)]
pub struct Version { history: String, prices: i64, producers:Value }
impl PartialEq for Version {
    fn eq(&self,other:&Self)->bool {self.history==other.history && self.prices==other.prices && crate::engine::model::stamp_set_current_for(&self.producers,&other.producers)}
}
pub fn version(c:&Connection,owner:i64)->Result<Version,FiguresError> {
    let origin=crate::sync::cache::capture_read(c,owner,None).map_err(|_|FiguresError::Data("ledger cache provenance unavailable".into()))?;
    Ok(Version { history:backfill::history_version(c,owner)?, prices:backfill::generation(c)?,producers:origin.producer_stamps() })
}
pub fn key(owner:i64)->String { format!("rust_tracker_ledger.{owner}") }
fn encode(s:&Summary)->Value {
    json!({"positions":s.positions.iter().map(|p|json!({"symbol":p.symbol,"quantity":p.quantity.to_s_f(),"cost":p.cost.to_s_f(),"avg_cost":p.avg_cost.to_s_f(),"estimated":p.estimated,"unpriced":p.unpriced.to_s_f()})).collect::<Vec<_>>(),
        "total_invested":s.total_invested.to_s_f(),"cash":s.cash.iter().map(|(k,v)|json!([k,v.to_s_f()])).collect::<Vec<_>>(),"cash_basis":s.cash_basis.iter().map(|(k,v)|json!([k,v.to_s_f()])).collect::<Vec<_>>(),"incomplete":s.incomplete,"loss_sales":s.loss_sales.iter().map(|(k,v)|json!([k,v.to_string()])).collect::<Vec<_>>()})
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
fn decode(v:&Value)->Result<Summary,FiguresError> {
    let pairs=|v:&Value|array(v)?.iter().map(|p|Ok((text(&p[0])?.into(),decimal(&p[1])?))).collect::<Result<Vec<_>,FiguresError>>();
    Ok(Summary { positions:array(&v["positions"] )?.iter().map(|p|Ok(Position { symbol:text(&p["symbol"] )?.into(),quantity:decimal(&p["quantity"] )?,cost:decimal(&p["cost"] )?,avg_cost:decimal(&p["avg_cost"] )?,estimated:boolean(&p["estimated"] )?,unpriced:decimal(&p["unpriced"] )? })).collect::<Result<_,FiguresError>>()?,
        total_invested:decimal(&v["total_invested"] )?,cash:pairs(&v["cash"] )?,cash_basis:pairs(&v["cash_basis"] )?,incomplete:boolean(&v["incomplete"] )?,loss_sales:array(&v["loss_sales"] )?.iter().map(|p|Ok((text(&p[0])?.into(),NaiveDate::parse_from_str(text(&p[1])?,"%Y-%m-%d").map_err(|_|malformed())?))).collect::<Result<_,FiguresError>>()? })
}
/// Called within the producer's write transaction; changed inputs cannot publish as current.
pub fn publish(c:&crate::engine::model::FencedTransaction<'_>,owner:i64,before:&Version,walked:Option<&Walked>,venue:i64,now:DateTime<Utc>)->Result<bool,FiguresError> {
    if &version(c,owner)? != before { return Ok(false); }
    let origin=crate::sync::cache::capture_read(c,owner,None).map_err(|_|FiguresError::Data("ledger cache provenance unavailable".into()))?;
    if !origin.ledger_is_current(c).map_err(|_|FiguresError::Data("ledger cache producer unavailable".into()))? {return Ok(false)}
    let payload=json!({"schema":1,"owner":owner,"history":before.history,"prices":before.prices,"producers":before.producers,"expires":(now+Duration::days(TTL_DAYS)).to_rfc3339(),"whole":walked.map(|w|encode(&w.whole)),"venue":walked.and_then(|w|w.venue.as_ref()).map(|s|json!([venue,encode(s)]))}).to_string();
    if payload.len()>MAX_PAYLOAD {return Err(FiguresError::NotComputed("ledger cache exceeds its size limit".into()));}
    app_config::set_plain(c,&key(owner),&payload,now).map_err(FiguresError::Data)?;
    Ok(true)
}
/// Cold includes absent/expired/obsolete/malformed cache entries, as Rails' shape guard does.
/// SQL failures propagate; a computed failure has its own safe, stated reason.
pub fn read(c:&Connection,owner:i64,exchange:Option<i64>,now:DateTime<Utc>)->Result<State,FiguresError> {
    if c.is_autocommit(){let tx=c.unchecked_transaction()?;let out=read(&tx,owner,exchange,now)?;tx.commit()?;return Ok(out)}
    let Some(raw)=app_config::get_plain(c,&key(owner)).map_err(FiguresError::Data)? else {return Ok(State::Cold)};
    if raw.len()>MAX_PAYLOAD {crate::engine::log("[tracker] ledger cache cold: payload exceeds 8388608-byte limit");return Ok(State::Cold)}
    let Ok(v)=serde_json::from_str::<Value>(&raw) else {return Ok(State::Cold)};
    if v["schema"].as_u64()!=Some(1) || v["owner"].as_i64()!=Some(owner) {return Ok(State::Cold)}
    let Some(expires)=v["expires"].as_str().and_then(|s|s.parse::<DateTime<Utc>>().ok()) else {return Ok(State::Cold)};
    if now>=expires {return Ok(State::Cold)}
    let current=version(c,owner)?;
    if v["history"].as_str()!=Some(current.history.as_str()) || v["prices"].as_i64()!=Some(current.prices) {return Ok(State::Cold)}
    if !crate::engine::model::stamp_set_current_for(&v["producers"],&current.producers) {return Ok(State::Cold)}
    let origin=crate::sync::cache::capture_read(c,owner,None).map_err(|_|FiguresError::Data("ledger cache provenance unavailable".into()))?;
    if !origin.ledger_is_current(c).map_err(|_|FiguresError::Data("ledger cache producer unavailable".into()))? {return Ok(State::Cold)}
    if !v.as_object().is_some_and(|o|o.contains_key("whole") && o.contains_key("venue")) {return Ok(State::Cold)}
    if v["whole"].is_null() && v["venue"].is_null() {return Ok(State::Failed(UNAVAILABLE))}
    type Scopes=(Summary,Option<(i64,Summary)>);
    let decoded=(||->Result<Scopes,FiguresError>{
        let whole=decode(&v["whole"])?;
        let venue=if v["venue"].is_null() {
            if encode(&whole)!=encode(&Summary::empty()) {return Err(malformed())}
            None
        } else {
            let pair=array(&v["venue"])?;
            if pair.len()!=2 {return Err(malformed())}
            Some((pair[0].as_i64().ok_or_else(malformed)?,decode(&pair[1])?))
        };
        Ok((whole,venue))
    })();
    match decoded {
        Ok((whole,venue))=>Ok(State::Warm(Box::new(match exchange {
            None=>whole,
            Some(id)=>match venue {Some((venue,summary)) if venue==id=>summary,_=>Summary::empty()},
        }))),
        Err(_)=>Ok(State::Cold),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn publish(c:&Connection,owner:i64,before:&Version,walked:Option<&Walked>,venue:i64,now:DateTime<Utc>)->Result<bool,FiguresError>{
        let tx=c.unchecked_transaction()?;let fenced=crate::engine::model::fence_versions(&tx,&[]).map_err(|_|malformed())?;
        let out=super::publish(&fenced,owner,before,walked,venue,now)?;tx.commit()?;Ok(out)
    }
    fn db()->Connection {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE api_keys(id INTEGER,user_id INTEGER,exchange_id INTEGER,key_type INTEGER,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT); CREATE TABLE app_configs(key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT); CREATE TABLE account_transactions(id INTEGER,user_id INTEGER,updated_at TEXT); CREATE TABLE historical_prices(id INTEGER); CREATE TABLE account_balances(id INTEGER,usd_value NUMERIC);").unwrap(); c
    }
    fn now()->DateTime<Utc> {"2026-10-08T12:00:00Z".parse().unwrap()}
    fn walked()->Walked {
        let mut whole=Summary::empty(); whole.total_invested=Dec::strict("123.4567890123456789").unwrap();
        whole.positions.push(Position{symbol:"AAPL".into(),quantity:Dec::strict("2.5").unwrap(),cost:Dec::strict("123.4567890123456789").unwrap(),avg_cost:Dec::strict("49.38271560493827156").unwrap(),estimated:true,unpriced:Dec::zero()});
        whole.cash=vec![("USD".into(),Dec::strict("-0.0").unwrap())];whole.cash_basis=vec![("USD".into(),Dec::strict("1.01").unwrap())];whole.incomplete=true;whole.loss_sales=vec![("AAPL".into(),now().date_naive())];
        Walked{venue:Some(whole.clone()),whole,terms:vec![]}
    }
    fn warm(c:&Connection,exchange:Option<i64>)->Summary {
        let State::Warm(s)=read(c,1,exchange,now()).unwrap() else { panic!("expected warm cache") };*s
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
        c.execute_batch("INSERT INTO account_transactions VALUES(2,42,'2026-10-09'); INSERT INTO account_balances VALUES(1,100000)").unwrap();warm(&c,None);
        assert!(matches!(read(&c,1,None,now()+Duration::days(30)-Duration::microseconds(1)).unwrap(),State::Warm(_)));
        assert!(matches!(read(&c,1,None,now()+Duration::days(30)).unwrap(),State::Cold));
    }
    #[test]
    fn cache_malformed_scope_never_becomes_a_numeric_empty_summary() {
        let c=db();let v=version(&c,1).unwrap();
        for (field,value) in [("venue",serde_json::Value::Null),("venue",serde_json::json!("bad")),("venue",serde_json::json!([])),("venue",serde_json::json!(["7",{}])),("venue",serde_json::json!([7,{}])),("whole",serde_json::json!({}))] {
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
    }
    #[test]
    fn cache_failure_malformed_schema_and_bad_decimals_never_return_money() {
        let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,None,7,now()).unwrap();
        assert!(matches!(read(&c,1,None,now()).unwrap(),State::Failed(UNAVAILABLE)));
        for raw in ["not JSON","null","{}"] {app_config::set_plain(&c,&key(1),raw,now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));}
        for (field,value) in [("schema",serde_json::json!(0)),("owner",serde_json::json!(42))] {
            publish(&c,1,&v,Some(&walked()),7,now()).unwrap();let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw[field]=value;app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        }
        publish(&c,1,&v,Some(&walked()),7,now()).unwrap();let mut raw:serde_json::Value=serde_json::from_str(&app_config::get_plain(&c,&key(1)).unwrap().unwrap()).unwrap();raw["whole"]["total_invested"]=serde_json::json!("NaN");app_config::set_plain(&c,&key(1),&raw.to_string(),now()).unwrap();assert!(matches!(read(&c,1,None,now()).unwrap(),State::Cold));
        c.execute_batch("DROP TABLE app_configs").unwrap();assert!(read(&c,1,None,now()).is_err());
    }

    #[test]
    fn cache_real_split_walk_roundtrips_long_computed_decimals() {
        use crate::{figures::at::At,tracker::{rows::Kind,walk::{self,Row}}};
        let row=|kind,amount:&str,value:&str|Row{kind,base:"AAPL".into(),amount:Dec::strict(amount).unwrap(),quote:Some("USD".into()),quote_amount:Some(Dec::strict(value).unwrap()),fiat_value:Dec::strict(value).unwrap(),at:At::from_utc(now()).unwrap(),price_missing:false,linked:false,transfer_fee:None,per_share:None,opening:false};
        let mut rows=vec![row(Kind::Buy,"9","100")];
        for _ in 0..3 {rows.push(row(Kind::Adjustment,"2","0"));}
        let w=walk::walk(&rows,0,now().date_naive()).unwrap();
        let longest=[&w.whole.positions[0].quantity,&w.whole.positions[0].cost,&w.whole.positions[0].avg_cost].into_iter().map(|d|d.to_s_f().len()).max().unwrap();
        assert!(longest==287,"three split adjustments must exercise the 287-character regression, got {longest}: {:?}",encode(&w.whole));
        let c=db();let v=version(&c,1).unwrap();publish(&c,1,&v,Some(&w),7,now()).unwrap();
        c.pragma_update(None,"query_only",true).unwrap();let before=c.total_changes();
        for (scope,expected) in [(None,&w.whole),(Some(7),w.venue.as_ref().unwrap())] {
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
