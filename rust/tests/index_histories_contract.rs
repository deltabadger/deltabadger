//! Rails-free drift check for the B2b index-history oracle (script/rust/index_histories.rb).
use sha2::{Digest, Sha256};

#[test]
fn the_index_history_oracle_pins_the_rails_it_was_recorded_from() {
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/index_histories.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let pins = v["ported_sources"].as_object().unwrap();
    assert_eq!(pins.len(), 18);
    for (path, hash) in pins {
        let got = format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap()));
        assert_eq!(&got, hash.as_str().unwrap(), "{path} changed: re-record script/rust/index_histories.rb and re-check the port");
    }
    assert_eq!(v["scenarios"].as_array().unwrap().len(), 38);
}
