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

/// D-MCP-COND-KINDS: update_bot_settings' fields of the rules an index or signal bot runs here (conditions::support: the cap,
/// the buy-side triggers, and a signal bot's sell-side triggers) say so; nothing else of the recorded schemas changes.
#[test]
fn the_rule_kinds_overlay_is_the_only_change_to_the_recorded_schemas(){
    let (mut recorded,mut ours)=(protocol::recorded(),protocol::metadata().clone());
    // D-SPLIT-API: split_bots, this build's own, is listed after the recorded tools (pinned below).
    let split=ours["tools"].as_array_mut().unwrap().pop().unwrap();
    assert_eq!(split["name"],"split_bots");
    ours["tools"].as_array_mut().unwrap().retain(|t| !["list_rules","create_rule","update_rule_settings","start_rule","stop_rule","delete_rule"].contains(&t["name"].as_str().unwrap()));
    let settings=|v:&Value|v["tools"].as_array().unwrap().iter().position(|t|t["name"]=="update_bot_settings").unwrap();
    let at=settings(&ours);
    let wanted=ours["tools"][at]["inputSchema"]["properties"].clone();
    let mut changed=vec![];
    for (name,schema) in recorded["tools"][at]["inputSchema"]["properties"].as_object_mut().unwrap().iter_mut(){
        if schema["description"]!=wanted[name]["description"]{changed.push(name.clone());schema["description"]=wanted[name]["description"].clone();}
    }
    assert_eq!(recorded,ours,"only descriptions of update_bot_settings change");
    assert_eq!(changed, [
        "quote_amount_limited", "quote_amount_limit",
        "price_limited", "price_limit_mode", "price_limit_value_condition", "price_limit", "price_limit_range_lower_bound", "price_limit_range_upper_bound", "price_limit_in_ticker_id",
        "price_drop_limited", "price_drop_limit_mode", "price_drop_limit", "price_drop_limit_time_window_condition", "price_drop_limit_in_ticker_id",
        "moving_average_limited", "moving_average_limit_mode", "moving_average_limit_value_condition", "moving_average_limit_in_ma_type", "moving_average_limit_in_timeframe", "moving_average_limit_in_period", "moving_average_limit_in_ticker_id",
        "indicator_limited", "indicator_limit_mode", "indicator_limit_value_condition", "indicator_limit", "indicator_limit_in_indicator", "indicator_limit_in_timeframe", "indicator_limit_in_ticker_id",
        "sell_price_limited", "sell_price_limit_mode", "sell_price_limit_value_condition", "sell_price_limit", "sell_price_limit_range_lower_bound", "sell_price_limit_range_upper_bound",
        "sell_price_drop_limited", "sell_price_drop_limit_mode", "sell_price_drop_limit", "sell_price_drop_limit_time_window_condition",
        "sell_moving_average_limited", "sell_moving_average_limit_mode", "sell_moving_average_limit_value_condition", "sell_moving_average_limit_in_ma_type", "sell_moving_average_limit_in_timeframe", "sell_moving_average_limit_in_period",
        "sell_indicator_limited", "sell_indicator_limit_mode", "sell_indicator_limit_value_condition", "sell_indicator_limit", "sell_indicator_limit_in_indicator", "sell_indicator_limit_in_timeframe",
    ]);
    // D-SIG-CONDITIONS / D-SIG-CAP: a signal bot takes the cap and both sides' trigger rules, never a subject.
    let field=|name:&str|ours["tools"][at]["inputSchema"]["properties"][name]["description"].as_str().unwrap().to_string();
    assert!(field("quote_amount_limit").ends_with(" Basket, index and signal bots."),"{}",field("quote_amount_limit"));
    assert!(field("sell_price_limit_mode").ends_with(" Basket and signal bots; flip is for basket bots only."),"{}",field("sell_price_limit_mode"));
    let ticker=&ours["tools"].as_array().unwrap().iter().find(|t|t["name"]=="update_bot_settings").unwrap()["inputSchema"]["properties"]["price_limit_in_ticker_id"]["description"];
    assert_eq!(ticker,"Price condition while buying. Basket and index bots: the ticker id of the member it watches (get_bot_details lists them); an index bot's may be a former member.");
}

/// D-SPLIT-API: split_bots' schema, alongside A1's six listed additions: the bots to split, and the bots whose money from
/// past sales stays as it is.
#[test]
fn split_bots_schema_is_pinned(){
    let tools=protocol::metadata()["tools"].as_array().unwrap();
    assert_eq!(tools.last().unwrap()["name"],"split_bots");
    assert_eq!(tools.len(),protocol::recorded()["tools"].as_array().unwrap().len()+6+1, "six A1 rule tools and split_bots are listed additions");
    assert_eq!(tools.last().unwrap()["inputSchema"],serde_json::json!({
        "$schema":"https://json-schema.org/draft/2020-12/schema","type":"object",
        "properties":{
            "bot_ids":{"type":"array","items":{"type":"integer"},"description":"The IDs of the bots to split"},
            "keep_proceeds_for":{"type":"array","items":{"type":"integer"},"description":"The IDs of the bots whose money from past sales stays as it is (No to the redeploy offer)"}
        },
        "required":["bot_ids"],"additionalProperties":false
    }));
    assert!(tools.last().unwrap()["description"].as_str().unwrap().contains("proceeds_unanswered"));
}

/// A1 listed additions: Rails property types and required fields, plus the W2 matrix fields.
#[test]
fn withdrawal_rule_schemas_and_permissions_are_pinned() {
    use deltabadger::web::{mcp::tools::NAMES, oauth::{TOOL_DEFAULTS,TOOL_GROUPS}};
    let catalog=protocol::metadata()["tools"].as_array().unwrap();
    let common=[("withdrawal_percentage","number"),("max_fee_percentage","number"),("min_amount","number"),("threshold_type","string"),("max_interval","integer"),("address","string"),("network","string"),("address_tag","string"),("address_name","string")];
    for name in ["list_rules","create_rule","update_rule_settings","start_rule","stop_rule","delete_rule"] {
        assert!(NAMES.contains(&name));
        assert_eq!(TOOL_DEFAULTS.iter().find(|(n,_)|*n==name).unwrap().1,name=="list_rules");
        assert!(TOOL_GROUPS.iter().find(|(n,_)|*n==if name=="list_rules" {"read"} else {"control"}).unwrap().1.contains(&name));
        let tool=catalog.iter().find(|t|t["name"]==name).unwrap();let schema=&tool["inputSchema"];
        assert_eq!(schema["$schema"],"https://json-schema.org/draft/2020-12/schema");assert_eq!(schema["type"],"object");assert_eq!(schema["additionalProperties"],false);
        let mut expected=serde_json::Map::new();
        if name=="create_rule" {for field in ["exchange_name","asset"] {expected.insert(field.into(),serde_json::json!("string"));}}
        if name!="list_rules" && name!="create_rule" {expected.insert("rule_id".into(),serde_json::json!("number"));}
        if name=="create_rule" || name=="update_rule_settings" {for (field,kind) in common {expected.insert(field.into(),serde_json::json!(kind));}}
        if name=="create_rule" || name=="update_rule_settings" { expected.insert("max_interval".into(),serde_json::json!(["integer","null"])); }
        let actual:serde_json::Map<_,_>=schema["properties"].as_object().unwrap().iter().map(|(k,v)|(k.clone(),v["type"].clone())).collect();
        assert_eq!(actual,expected,"{name}");
        assert_eq!(schema["required"],match name {"list_rules"=>Value::Null,"create_rule"=>serde_json::json!(["exchange_name","asset","address"]),_=>serde_json::json!(["rule_id"])});
        assert_eq!(tool["annotations"],if name=="list_rules" {serde_json::json!({"readOnlyHint":true})} else {Value::Null});
        let descriptions = [
            ("rule_id", "The rule ID"), ("exchange_name", "Exchange name (e.g., Binance)"), ("asset", "Asset symbol to withdraw (e.g., BTC)"),
            ("address", "Destination address, exactly as allow-listed on the exchange"), ("address_tag", "Memo / tag, if the network needs one"),
            ("network", "Network, if the exchange needs one"), ("address_name", "The listed withdrawal key for a Kraken destination"),
            ("max_interval", "Maximum interval in days; a positive integer, or null to remove the deadline"),
            ("withdrawal_percentage", if name=="create_rule" {"Share of the free balance to withdraw (default 100)"} else {"Percentage of available balance to withdraw (optional)"}),
            ("threshold_type", if name=="create_rule" {"'fee_percentage' (default) or 'min_amount'"} else {"'fee_percentage' or 'min_amount' — which threshold to use (optional)"}),
            ("max_fee_percentage", if name=="create_rule" {"Withdraw only when the fee is at most this percent (default 0.5)"} else {"Max acceptable withdrawal fee percentage (optional)"}),
            ("min_amount", if name=="create_rule" {"Withdraw only above this amount (for threshold_type 'min_amount')"} else {"Minimum amount threshold (optional)"}),
        ];
        let properties:serde_json::Map<_,_>=expected.into_iter().map(|(field,kind)| {
            let description=descriptions.iter().find(|(n,_)|*n==field).unwrap().1;
            (field,serde_json::json!({"type":kind,"description":description}))
        }).collect();
        let mut want=serde_json::json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":properties,"additionalProperties":false});
        if name!="list_rules" {want["required"]=if name=="create_rule" {serde_json::json!(["exchange_name","asset","address"])} else {serde_json::json!(["rule_id"])};}
        assert_eq!(*schema,want,"the complete {name} schema");
    }
}
