//! Ruby-free source drift and recorded decision contract.
use serde_json::Value;
use sha2::{Digest,Sha256};

#[test]
fn sources_and_history_vectors_are_reviewable() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let bytes=std::fs::read(root.join("rust/tests/fixtures/histories.json")).expect("record the synthetic history vectors first");
    let v:Value=serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["synthetic_only"],true);
    assert_eq!(v["cases"].as_array().unwrap().len(),24);
    for (name,hash) in v["ported_sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(name)).unwrap())),hash.as_str().unwrap(),"{name}: re-record");
    }
    for name in ["null_price_buy","cancelled_partial_buy"] {
        let c=v["cases"].as_array().unwrap().iter().find(|c|c["name"]==name).unwrap();
        assert_eq!(c["rails_unchanged"]["orders"][0]["quote"],"75.0");
        assert_eq!(c["normalized"]["orders"][0]["quote"],"60.0");
        assert_eq!(c["rails_unchanged"]["contributed"],"70.0");
        assert_eq!(c["normalized"]["contributed"],"100.0");
    }
    for (name,first,later) in [("merge_before_start","100.0","200.0"),("merge_at_start","0.0","100.0")] {
        let c=v["cases"].as_array().unwrap().iter().find(|c|c["name"]==name).unwrap();
        assert_eq!(c["rails_unchanged"],c["normalized"]);
        assert_eq!(c["normalized"]["pending"],first);
        assert_eq!(c["one_week_pending"],later);
        assert_eq!(c["one_week_rails_pending"],later);
    }
}
