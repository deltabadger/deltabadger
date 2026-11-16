//! The read boundary's production integration.
#[test]
fn tracker_index_keeps_the_read_boundary_and_wakes_jobs_only_after_it() {
    let source=include_str!("../src/web/tracker/index.rs");
    let (read,after)=source.split_once("app.db(move|c|super::read::only(c,|c|{").expect("the index reads inside read::only");
    let (inside,after)=after.split_once("})).await?;").expect("the read closure ends before anything else");
    assert!(inside.contains("render(c,&ctx,owner,&fx)") && inside.contains("page.status()==StatusCode::OK"));
    assert!(!read.contains("wake_job(") && !inside.contains("wake_job("),"a job is woken only after the read returned");
    assert!(after.contains("app.wake_job(name,&scope)"));
    for forbidden in ["wake_engine(",".execute(",".execute_batch(",".unwrap(",".expect(","amount_exec","quote_amount_exec"] {
        assert!(!source.contains(forbidden),"read route includes {forbidden}");
    }
}
