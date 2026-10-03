//! Rails-free proof that page formatting and terminal states retain the figures' budget in release.
use deltabadger::figures::{at::At,budget::{self,Limits,FIGURE},scripted::Scripted,FiguresError,OVER_BUDGET};
use deltabadger::web::figure::{self,loading::{self,Snapshot}};
use rusqlite::Connection;
use serde_json::json;
const SCHEMA: &str = "
    CREATE TABLE users (id integer PRIMARY KEY, time_zone varchar, display_currency varchar, hide_balances boolean, wash_sale_enabled boolean DEFAULT 0, wash_sale_jurisdiction varchar);
    CREATE TABLE exchanges (id integer PRIMARY KEY, type varchar);
    CREATE TABLE assets (id integer PRIMARY KEY, symbol varchar, name varchar, category varchar, external_id varchar, image_url varchar, color varchar);
    CREATE TABLE tickers (id integer PRIMARY KEY, exchange_id integer, ticker varchar, base varchar, base_asset_id integer, quote_asset_id integer,
                          quote_decimals integer, available boolean, trading_enabled boolean, minimum_base_size decimal DEFAULT 0.000000001, minimum_quote_size decimal DEFAULT 1);
    CREATE TABLE bots (id integer PRIMARY KEY, user_id integer, exchange_id integer, type varchar, settings json, status integer, transient_data json DEFAULT '{}', redeploy_declined_offset decimal DEFAULT 0);
    CREATE TABLE bot_index_assets (bot_id integer, asset_id integer, ticker_id integer, in_index boolean);
    CREATE TABLE transactions (id integer PRIMARY KEY, bot_id integer, status integer, created_at datetime(6), exchange_id integer, price decimal, amount decimal,
                               amount_exec decimal, quote_amount_exec decimal, base varchar, base_asset_id integer, side integer, external_status integer,
                               transaction_type varchar);
    CREATE TABLE account_transactions (id integer PRIMARY KEY, user_id integer, exchange_id integer, entry_type integer, base_currency varchar, raw_data json, transacted_at datetime(6));
    INSERT INTO users(id,time_zone,display_currency,hide_balances) VALUES (1, 'UTC', 'USD', 0);
    INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca');
    INSERT INTO assets(id,symbol,name,category,external_id) VALUES (1, 'USD', 'USD', 'Fiat', 'usd'), (2, 'AAA', 'AAA Inc.', 'Stock', 'stock-aaa'), (3, 'BBB', 'BBB Inc.', 'Stock', 'stock-bbb');
    INSERT INTO tickers(id,exchange_id,ticker,base,base_asset_id,quote_asset_id,quote_decimals,available,trading_enabled) VALUES (1, 1, 'AAA', 'AAA', 2, 1, 2, 1, 1), (2, 1, 'BBB', 'BBB', 3, 1, 2, 1, 1);
    INSERT INTO bots(id,user_id,exchange_id,type,settings,status) VALUES (1, 1, 1, 'Bots::DcaMultiAsset', '{\"quote_asset_id\": 1, \"allocations\": {\"2\": 0.5, \"3\": 0.5}}', 1);
";

pub fn run()->Result<(),String>{
    let c=Connection::open_in_memory().map_err(|e|e.to_string())?;
    c.execute_batch(SCHEMA).map_err(|e|e.to_string())?;
    c.execute_batch("INSERT INTO transactions VALUES(1,1,0,'2026-03-02 14:30:00',1,100,1,1,100,'AAA',2,0,2,'REGULAR');").map_err(|e|e.to_string())?;
    let script=json!({"GET data.alpaca.markets/v2/stocks/snapshots":{"body":{"AAA":{"latestTrade":{"p":104}}}},
        "GET data.alpaca.markets/v2/stocks/AAA/bars":{"body":{"bars":[]}},
        "GET data.alpaca.markets/v2/stocks/AAA/bars?adjustment=split":{"body":{"bars":[]}}});
    let market=Scripted::new(&script,None);let now=At(1_773_000_000_000_000_000);
    let (good,used)=budget::scope(FIGURE,||figure::account(&c,1,&market,now,"en","token",""));
    let good=good.map_err(|e|format!("{e:?}"))?;
    if !good["bots"]["1"]["tile"].as_str().is_some_and(|html|html.contains("4.00%")){return Err("missing normal figure".into());}
    if used.steps<good.to_string().len() as u64||used.held==0{return Err("page bypassed the budget".into());}
    for limits in [Limits{steps:0,held:FIGURE.held},Limits{steps:used.steps-1,held:FIGURE.held},Limits{steps:FIGURE.steps,held:0}] {
        let (refused,_)=budget::scope(limits,||figure::account(&c,1,&market,now,"en","token",""));
        if !matches!(refused,Err(FiguresError::NotComputed(ref message)) if message==OVER_BUDGET){return Err(format!("page escaped {limits:?}"));}
    }
    let failed=loading::render(&c,1,&Snapshot::Failed,"en","token","").ok_or("terminal state missing")?;
    for part in ["tile","metrics","chart"] {
        let html=failed["bots"]["1"][part].as_str().ok_or("part missing")?;
        if !html.contains("no-value")||html.contains("loader")||html.contains("0.00"){return Err(format!("dishonest terminal {part}"));}
    }
    let (_,after)=budget::scope(FIGURE,||figure::account(&c,1,&market,now,"en","token",""));
    if after!=used{return Err("a prior render leaked its decimal meter".into());}
    Ok(())
}
#[allow(dead_code)]
fn main(){match run(){Ok(())=>println!("page figures and serialization keep the budget; failures show no value"),Err(e)=>{eprintln!("{e}");std::process::exit(1);}}}
