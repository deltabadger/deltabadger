//! The B2b index-history oracle (script/rust/index_histories.rb) holds every scenario the port is checked on.

#[test]
fn the_index_history_oracle_holds_every_scenario() {
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/index_histories.json")).unwrap();
    assert_eq!(v["scenarios"].as_array().unwrap().len(), 42);
}
