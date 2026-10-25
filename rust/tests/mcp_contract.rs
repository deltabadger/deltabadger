use deltabadger::web::mcp::protocol;
use serde_json::Value;
#[test]
fn schema_errors_match_recorded_json_schemer_vectors(){
    let cases:Vec<Value>=serde_json::from_str(include_str!("fixtures/mcp_validation.json")).unwrap();
    assert!(cases.len()>300);
    let mut failures=vec![];
    for (i,case) in cases.iter().enumerate(){
        let name=case["schema"].as_str().unwrap();
        let schema=protocol::metadata()["requests"].get(name).unwrap_or(&protocol::metadata()["notifications"][name]);
        let got=protocol::validate(&case["value"],schema,"");
        if serde_json::json!(got)!=case["errors"] {if failures.len()<8{eprintln!("{name} case {i} value {} got {:?} want {}",case["value"],got,case["errors"]);}failures.push(i);}
    }
    assert!(failures.is_empty(),"{} mismatching schema vectors",failures.len());
}
#[test]
fn schema_contains_exactly_the_implemented_tools(){
    let names:Vec<_>=protocol::metadata()["tools"].as_array().unwrap().iter().map(|v|v["name"].as_str().unwrap()).collect();
    assert_eq!(names,deltabadger::web::mcp::tools::NAMES);
}
#[test]
fn recorded_sources_have_not_drifted(){
    use sha2::{Digest,Sha256};
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let sources=protocol::metadata()["sources"].as_object().unwrap();assert!(sources.len()>25);
    for (path,want) in sources{assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap())),want.as_str().unwrap(),"{path}: re-record MCP metadata and rerun parity");}
}

#[test]
fn m3_reads_remain_registered_at_every_m4_task_boundary(){
    for name in ["list_bots","get_bot_details","list_exchanges","get_exchange_balances","get_portfolio_summary","list_transactions","list_open_orders","list_tax_jurisdictions"] {
        assert!(deltabadger::web::mcp::tools::NAMES.contains(&name),"M3 registry retained: {name}");
    }
}

/// D-MCP-COND-KINDS: update_bot_settings' fields of the rules an index bot runs here (conditions::support: the cap and the
/// buy side's price rule) say so; nothing else of the recorded schemas changes.
#[test]
fn the_rule_kinds_overlay_is_the_only_change_to_the_recorded_schemas(){
    let (mut recorded,ours)=(protocol::recorded(),protocol::metadata().clone());
    let settings=|v:&Value|v["tools"].as_array().unwrap().iter().position(|t|t["name"]=="update_bot_settings").unwrap();
    let at=settings(&ours);
    let wanted=ours["tools"][at]["inputSchema"]["properties"].clone();
    let mut changed=vec![];
    for (name,schema) in recorded["tools"][at]["inputSchema"]["properties"].as_object_mut().unwrap().iter_mut(){
        if schema["description"]!=wanted[name]["description"]{changed.push(name.clone());schema["description"]=wanted[name]["description"].clone();}
    }
    assert_eq!(recorded,ours,"only descriptions of update_bot_settings change");
    assert_eq!(changed,["quote_amount_limited","quote_amount_limit","price_limited","price_limit_mode","price_limit_value_condition","price_limit","price_limit_range_lower_bound","price_limit_range_upper_bound","price_limit_in_ticker_id"]);
    let ticker=&ours["tools"].as_array().unwrap().iter().find(|t|t["name"]=="update_bot_settings").unwrap()["inputSchema"]["properties"]["price_limit_in_ticker_id"]["description"];
    assert_eq!(ticker,"Price condition while buying. Basket and index bots: the ticker id of the member it watches (get_bot_details lists them); an index bot's may be a former member.");
}
