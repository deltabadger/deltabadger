//! Rails-free source drift and the read boundary's production integration.
use sha2::{Digest,Sha256};
#[test]
fn tracker_index_oracle_sources_stay_pinned() {
    let pins:std::collections::BTreeMap<String,String>=serde_json::from_str(include_str!("fixtures/tracker_index_sources.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for(path,hash)in pins {assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(&path)).unwrap())),hash,"{path}: record the Rails grid again");}
}
#[test]
fn tracker_index_keeps_the_read_boundary_and_wakes_jobs_only_after_it() {
    let source=include_str!("../src/web/tracker/index.rs");
    let (read,after)=source.split_once("app.db(move|c|super::read::only(c,|c|{").expect("the index reads inside read::only");
    let (inside,after)=after.split_once("})).await?;").expect("the read closure ends before anything else");
    assert!(inside.contains("render(c,&ctx,owner,Some(&fx))") && inside.contains("page.status()==StatusCode::OK"));
    assert!(!read.contains("wake_job(") && !inside.contains("wake_job("),"a job is woken only after the read returned");
    assert!(after.contains("app.wake_job(name,&scope)"));
    for forbidden in ["wake_engine(",".execute(",".execute_batch(",".unwrap(",".expect(","amount_exec","quote_amount_exec"] {
        assert!(!source.contains(forbidden),"read route includes {forbidden}");
    }
}
