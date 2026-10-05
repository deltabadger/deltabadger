//! Rails-free permission and schema coverage for all four reads.
use deltabadger::web::{bearer::Bearer,mcp::{protocol,tools}};
use rusqlite::Connection;
use serde_json::json;
const READS:[&str;4]=["get_exchange_balances","list_open_orders","get_bot_details","get_portfolio_summary"];
fn database()->Connection{
    let c=Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE users(id INTEGER PRIMARY KEY,mcp_settings TEXT);CREATE TABLE oauth_applications(id INTEGER PRIMARY KEY,personal_access_token BOOLEAN,personal_owner_id INTEGER);CREATE TABLE connected_clients(user_id INTEGER,oauth_application_id INTEGER,mcp_tools TEXT);INSERT INTO users VALUES(1,'{}');INSERT INTO oauth_applications VALUES(1,0,NULL);").unwrap();c
}
#[test]
fn every_read_is_hidden_until_granted_and_removed_when_disabled(){
    let c=database();let who=Bearer{user_id:1,application_id:1,token_id:1};
    for name in READS{
        assert!(!tools::registry(&c,who).unwrap().iter().any(|n|n==name));
        assert_eq!(tools::gate(&c,who,name).unwrap().unwrap(),protocol::tool_text(&format!("Tool '{name}' is not available to this client. Grant it in Settings > Connect."),true));
    }
    c.execute("INSERT INTO connected_clients VALUES(1,1,?1)",[json!(READS).to_string()]).unwrap();
    assert_eq!(tools::registry(&c,who).unwrap().len(),4);
    for name in READS{
        c.execute("UPDATE users SET mcp_settings=?1",[json!({"tool_permissions":{name:false}}).to_string()]).unwrap();
        assert!(!tools::registry(&c,who).unwrap().iter().any(|n|n==name));
        assert_eq!(tools::gate(&c,who,name).unwrap().unwrap(),protocol::tool_text(&format!("Tool '{name}' is disabled. Enable it in Settings > MCP."),true));
    }
}
#[test]
fn required_arguments_and_unknown_fields_match_the_recorded_schemas(){
    for name in READS{
        let schema=&protocol::metadata()["tools"].as_array().unwrap().iter().find(|t|t["name"]==name).unwrap()["inputSchema"];
        assert!(!protocol::validate(&json!({"unknown":true}),schema,"").is_empty(),"{name}");
        let valid=match name{"get_bot_details"=>json!({"bot_id":1}),"get_exchange_balances"=>json!({"exchange_name":"Alpaca"}),_=>json!({})};
        assert!(protocol::validate(&valid,schema,"").is_empty());
        let absent=protocol::validate(&json!({}),schema,"");
        assert_eq!(absent.is_empty(),matches!(name,"list_open_orders"|"get_portfolio_summary"));
    }
}

#[test]
fn the_ci_command_includes_this_rails_free_binary(){
    assert!(include_str!("../../.github/workflows/rust.yml").contains("--test mcp_reads_contract"));
}

#[test]
fn every_row_cap_accepts_the_boundary_and_refuses_the_next_row(){
    use deltabadger::web::mcp::read_limits as l;
    for cap in [l::CATALOG,l::BOTS,l::ORDERS,100,1]{
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE bounded(id INTEGER PRIMARY KEY,value TEXT)").unwrap();
        c.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO bounded SELECT i,'x' FROM n",[cap]).unwrap();
        assert!(l::table(&c,"bounded","?1=1",cap,1,&mut 0).unwrap(),"at {cap}");
        c.execute("INSERT INTO bounded(value) VALUES('x')",[]).unwrap();
        assert!(!l::table(&c,"bounded","?1=1",cap,1,&mut 0).unwrap(),"over {cap}");
    }
    for cap in [l::LOCAL_ORDERS,l::VENUE_ORDERS]{assert!(l::count(cap,cap));assert!(!l::count(cap+1,cap));}
}
#[test]
fn stored_strings_are_bounded_in_bytes_before_they_are_loaded(){
    use deltabadger::web::mcp::read_limits as l;
    let c=Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE bounded(value TEXT)").unwrap();
    c.execute("INSERT INTO bounded VALUES(?1)",["é".repeat(l::STRING/2)]).unwrap();
    assert!(l::table(&c,"bounded","?1=1",1,1,&mut 0).unwrap());
    c.execute("UPDATE bounded SET value=value||'x'",[]).unwrap();
    assert!(!l::table(&c,"bounded","?1=1",1,1,&mut 0).unwrap());
}
#[test]
fn aggregate_stored_bytes_are_bounded(){
    use deltabadger::web::mcp::read_limits as l;
    let c=Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE bounded(value TEXT)").unwrap();
    c.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?1) INSERT INTO bounded SELECT ?2 FROM n",rusqlite::params![l::STORED_BYTES/l::STRING,"x".repeat(l::STRING)]).unwrap();
    assert!(l::table(&c,"bounded","?1=1",l::CATALOG,1,&mut 0).unwrap());
    c.execute("INSERT INTO bounded VALUES('x')",[]).unwrap();
    assert!(!l::table(&c,"bounded","?1=1",l::CATALOG,1,&mut 0).unwrap());
}
#[test]
fn final_text_is_bounded_in_bytes_without_truncation(){
    use deltabadger::web::mcp::read_limits as l;
    let exact="é".repeat(l::TEXT/2);
    assert_eq!(l::text(&exact),exact);
    assert_eq!(l::text(&(exact+"x")),"Read unavailable: response exceeds 65536 bytes");
}
#[test]
fn oracle_fingerprints_include_the_money_implementations(){
    let meta=protocol::metadata();
    for file in ["app/models/exchanges/alpaca.rb","app/models/bots/dca_single_asset/measurable.rb","app/models/bot/composition/measurable.rb"]{
        assert!(meta["sources"][file].is_string(),"{file}");
    }
}
#[test]
fn read_order_price_absence_does_not_change_engine_polling(){
    use deltabadger::venue::alpaca::{parse_order,parse_read_order};
    for kind in ["market","limit"]{
        let raw=json!({"filled_qty":"0","qty":"2","symbol":"AAA","type":kind});
        assert!(parse_order("x",&raw).unwrap().price.unwrap().is_zero());
        assert!(parse_read_order("x",&raw).unwrap().price.is_none());
    }
}
#[test]
fn preliminary_bot_work_consumes_the_figures_budget(){
    use deltabadger::{web::mcp::read_limits,figures::budget::{self,Limits}};
    budget::scope(Limits{steps:1,held:0},||{
        assert!(read_limits::charge_bot().is_ok());
        assert!(read_limits::charge_bot().is_err());
    });
}

#[test]
fn fill_completeness_covers_every_quantity_value_column_combination() {
    use deltabadger::figures::{at::At,db::Order,dec::Dec,fill};
    // Missing, zero, negative and positive in every numeric column; closed vs other external states,
    // both sides. db::orders admits submitted rows only, and maps only Closed to closed=true.
    let values=[None,Some(0),Some(-1),Some(2)];
    for closed in [false,true] { for sell in [false,true] {
      for amount in values { for exec in values { for price in values { for quote in values {
        let dec=|v:Option<i64>|v.map(Dec::from_i64);
        let row=Order{id:1,at:At(0),exchange_id:None,raw:fill::Raw::new(dec(price),dec(amount),dec(exec),dec(quote)),base:None,asset_id:None,sell,buy:!sell,closed,kind:"REGULAR".into()};
        let quantity=exec.or(if closed{amount}else{None}).unwrap_or(0);
        let complete=quantity<=0||price.unwrap_or(0)>0||quote.unwrap_or(0)>0;
        let parsed=fill::parse(&row);
        assert_eq!(parsed.is_ok(),complete,"closed={closed} sell={sell} amount={amount:?} exec={exec:?} price={price:?} quote={quote:?}");
        if complete {
            let parsed=parsed.unwrap();
            if quantity<=0 {assert!(parsed.is_none());} else {
                let parsed=parsed.unwrap();
                assert_eq!(parsed.quantity,Dec::from_i64(quantity));
                let value=if quote.unwrap_or(0)>0{quote.unwrap()}else{price.unwrap()*quantity};
                assert_eq!(parsed.value,Dec::from_i64(value));
            }
        }
      }}}}
    }}
}

#[test]
fn normalized_fill_multiplies_decimals_without_float_rounding() {
    use deltabadger::figures::{at::At,db::Order,dec::Dec,fill};
    let row=Order{id:1,at:At(0),exchange_id:None,raw:fill::Raw::new(Some(Dec::parse("9007199254740993.1").unwrap()),None,Some(Dec::parse("0.1").unwrap()),None),base:None,asset_id:None,sell:true,buy:false,closed:false,kind:"REGULAR".into()};
    let parsed=fill::parse(&row).unwrap().unwrap();
    assert_eq!(parsed.value.to_s_f(),"900719925474099.31");
}

#[test]
fn accounting_cannot_read_raw_fill_columns() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/figures");
    let roots=[root,std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/figure")];
    for entry in roots.iter().flat_map(|root|std::fs::read_dir(root).unwrap()) {
        let path=entry.unwrap().path();
        if path.extension().is_some_and(|v|v=="rs") && path.file_name().unwrap()!="fill.rs" {
            let code=std::fs::read_to_string(&path).unwrap();
            let production=code.split("#[cfg(test)]").next().unwrap();
            for column in ["amount_exec","quote_amount_exec"]{assert!(!production.contains(column),"raw fill SQL outside predicate: {}",path.display());}
            assert!(!code.contains(".raw.") && !code.contains(".raw;"),"raw fill column access outside predicate: {}",path.display());
            for name in ["amount_exec","quote_amount_exec","price","amount","cost"] {
                for receiver in ["order","o"] {
                    assert!(!code.contains(&format!("{receiver}.{name}")),"raw fill column access outside predicate: {}",path.display());
                }
            }
        }
    }
}

#[test]
fn generated_index_labels_cannot_load_unbounded_index_metadata(){
    use deltabadger::web::mcp::read_limits as l;
    let c=Connection::open_in_memory().unwrap();
    for name in ["indices","assets","tickers","exchange_assets","exchanges","users","api_keys","bots","transactions","bot_index_assets","wash_sale_locks","account_transactions"]{
        c.execute_batch(&format!("CREATE TABLE {name}(id INTEGER PRIMARY KEY,user_id INTEGER,bot_id INTEGER,value TEXT)")).unwrap();
    }
    c.execute("INSERT INTO indices VALUES(1,1,1,?1)",["x".repeat(l::STRING)]).unwrap();
    assert!(l::check(&c,1).unwrap());
    c.execute("UPDATE indices SET value=value||'x'",[]).unwrap();
    assert!(!l::check(&c,1).unwrap(),"index metadata must pass read admission before generated labels load it");
}
