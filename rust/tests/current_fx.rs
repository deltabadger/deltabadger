//! Ruling 8: shared FX arithmetic, usable rates, and no database effects. Rails-free.
use deltabadger::figures::{at::At,db::Ticker,dec::Dec,market::{Failure,Fetch,MarketData,Member,Quoted,Venue},num::Num,totals,walk::Metrics,FiguresError};
struct Feed(Vec<(String,Member<Num>)>,Option<Quoted>);
impl MarketData for Feed {
 fn prices(&self,_:&Venue,_:&[String])->Fetch<Vec<(String,Member<Dec>)>>{unreachable!()}
 fn candles(&self,_:&Venue,_:&Ticker,_:At,_:i64,_:bool)->Fetch<Vec<(At,Dec)>>{unreachable!()}
 fn exchange_rates(&self)->Fetch<Vec<(String,Member<Num>)>>{Ok(self.0.clone())}
 fn coin_price(&self,_:&str,_:&str)->Fetch<Quoted>{self.1.clone().ok_or(Failure::Failed("absent".into()))}
}
fn db()->rusqlite::Connection {
 let c=rusqlite::Connection::open_in_memory().unwrap();
 c.execute_batch("CREATE TABLE assets(symbol TEXT,category TEXT,external_id TEXT); INSERT INTO assets VALUES('BTC','Cryptocurrency','bitcoin');").unwrap();c
}
fn feed(a:Num,b:Num)->Feed{Feed(vec![("usd".into(),Ok(a)),("eur".into(),Ok(b))],None)}
#[test]
fn integer_and_float_provider_rates_follow_rails_float_then_bigdecimal() {
 let c=db();
 // Values measured with Ruby Float division followed by Float#to_d on the pinned base.
 for (a,b,expected) in [(Num::Int(100),Num::Int(80),"0.8"),(Num::Float(100.0),Num::Float(80.0),"0.8"),
  (Num::Int(3),Num::Int(2),"0.6666666666666666"),(Num::Int(80),Num::Int(100),"1.25"),
  (Num::Float(64123.456),Num::Float(55210.987),"0.8610107820763747"),
  (Num::Float(1.0),Num::Float(1.0000000000000002),"1.0"),
  (Num::Float(1.0000000000000002),Num::Float(1.0),"0.9999999999999998")] {
  let d=totals::denomination(&c,&feed(a,b),"EUR").unwrap();
  assert_eq!(d.currency,"EUR");assert_eq!(d.rate.to_s_f(),expected);
 }

 let d=totals::denomination(&c,&feed(Num::Int(100),Num::Int(80)),"EUR").unwrap();
 assert_eq!(Dec::strict("80").unwrap().div(&d.rate).unwrap().to_s_f(),"100.0");
}
#[test]
fn unusable_rates_are_unavailable_in_shared_denominations_and_totals() {
 let c=db();let mut m=Metrics::empty();m.total_amount_value_in_quote=Num::Int(20);m.total_quote_amount_invested=Num::Int(10);
 for f in [feed(Num::Int(100),Num::Int(0)),feed(Num::Int(0),Num::Int(80)),feed(Num::Int(-100),Num::Int(-80)),
 feed(Num::Float(f64::INFINITY),Num::Float(80.0)),feed(Num::Float(100.0),Num::Float(f64::NAN)),
 feed(Num::Float(f64::MAX),Num::Float(f64::MIN_POSITIVE)),feed(Num::Float(f64::MIN_POSITIVE),Num::Float(f64::MAX)),Feed(vec![],None)] {
  assert!(matches!(totals::denomination(&c,&f,"EUR"),Err(FiguresError::NotComputed(reason)) if reason=="Currency conversion unavailable"));
  assert!(matches!(totals::profit_in_usd(&c,&f,&mut totals::Rates::default(),Some("EUR"),&m),Err(FiguresError::NotComputed(reason)) if reason=="Currency conversion unavailable"));
 }
 assert_eq!(c.total_changes(),1);
}
#[test]
fn coin_fx_is_also_decimal_positive_and_strict() {
 let c=db();let mut m=Metrics::empty();m.total_amount_value_in_quote=Num::Int(20);m.total_quote_amount_invested=Num::Int(10);
 for q in [Quoted::Num(Num::Int(0)),Quoted::Num(Num::Int(-1)),Quoted::Text("80oops".into())] {
  assert!(matches!(totals::profit_in_usd(&c,&Feed(vec![],Some(q)),&mut totals::Rates::default(),Some("BTC"),&m),Err(FiguresError::NotComputed(_))));
 }
 assert_eq!(totals::profit_in_usd(&c,&Feed(vec![],Some(Quoted::Num(Num::Int(80)))),&mut totals::Rates::default(),Some("BTC"),&m).unwrap().unwrap().to_d().unwrap().to_s_f(),"800.0");
}

#[test]
fn numeric_string_fx_uses_the_shared_float_path() {
 use deltabadger::figures::scripted::Scripted;
 use serde_json::json;
 let c=db();
 for (usd,eur,expected) in [("100.0","80.0","0.8"),("  1e2  ","+8.0e1","0.8"),("64123.456","55210.987","0.8610107820763747")] {
  let script=json!({"GET data-api:3000/api/v1/exchange_rates":{"body":{"data":{"usd":{"value":usd},"eur":{"value":eur}}}}});
  let market=Scripted::new(&script,Some("deltabadger"));
  assert_eq!(totals::denomination(&c,&market,"EUR").unwrap().rate.to_s_f(),expected);
 }
 for bad in ["12abc",""," ","NaN","inf","1e999","0","-80","0x50","8_0"] {
  let script=json!({"GET data-api:3000/api/v1/exchange_rates":{"body":{"data":{"usd":{"value":"100.0"},"eur":{"value":bad}}}}});
  let market=Scripted::new(&script,Some("deltabadger"));
  assert!(matches!(totals::denomination(&c,&market,"EUR"),Err(FiguresError::NotComputed(reason)) if reason=="Currency conversion unavailable"),"{bad}");
 }
 assert_eq!(c.total_changes(),1);
}

#[test]
fn legacy_display_currency_uses_rails_normalizer_without_stripping_nonblank_codes() {
 let c=db();let f=feed(Num::Int(100),Num::Int(80));
 for raw in ["usd","Usd",""," \t\n","\u{3000}"] {
  let d=totals::denomination(&c,&f,raw).unwrap();assert_eq!(d.currency,"USD");assert_eq!(d.rate.to_s_f(),"1.0");
 }
 let d=totals::denomination(&c,&f,"eur").unwrap();assert_eq!(d.currency,"EUR");assert_eq!(d.rate.to_s_f(),"0.8");
 assert!(totals::denomination(&c,&f," eur ").is_err());
}
