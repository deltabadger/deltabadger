//! Rails-free source drift and the read boundary's production integration.
use sha2::{Digest,Sha256};
#[test]
fn tracker_index_oracle_sources_stay_pinned() {
    let pins:std::collections::BTreeMap<String,String>=serde_json::from_str(include_str!("fixtures/tracker_index_sources.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for(path,hash)in pins {assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(&path)).unwrap())),hash,"{path}: record the Rails grid again");}
}
#[test]
fn tracker_index_keeps_the_read_boundary_and_schedules_no_work() {
    let source=include_str!("../src/web/tracker/index.rs");
    assert!(source.contains("super::read::only(c,|c|{let fx=super::fx::held(&ctx.app,c,owner)?;render(c,&ctx,owner,Some(&fx))})"));
    for forbidden in ["wake_job(","wake_engine(",".execute(",".execute_batch(",".unwrap(",".expect(","amount_exec","quote_amount_exec"] {
        assert!(!source.contains(forbidden),"read route includes {forbidden}");
    }
}
