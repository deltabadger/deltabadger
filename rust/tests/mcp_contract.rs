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
fn schema_contains_exactly_the_four_implemented_tools(){
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
