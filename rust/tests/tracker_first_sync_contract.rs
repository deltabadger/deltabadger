//! Rails-free read-only integration contracts.
#[test]
fn first_sync_stays_inside_the_read_boundary_and_adds_no_money_or_credentials() {
    let index=include_str!("../src/web/tracker/index.rs");
    assert!(index.contains("app.db(move|c|super::read::only(c,|c|{") && index.contains("let page=render(c,&ctx,owner,&fx)?;"));
    let source=include_str!("../src/web/tracker/first_sync.rs").split("#[cfg(test)]").next().unwrap();
    for forbidden in ["wake_job(","wake_engine(",".execute(",".execute_batch(",".unwrap(",".expect(","amount_exec","quote_amount_exec","usd_value","decrypt("] {
        assert!(!source.contains(forbidden),"first-sync view includes {forbidden}");
    }
}

#[test]
fn completion_payloads_match_recorded_v2_jobs() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/tracker_completion.json")).unwrap();
    for owner in [1, 42] {
        let messages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = messages.clone();
        let notifications = deltabadger::jobs::notifications::Notifications::new(move |stream, payload| { captured.lock().unwrap().push(serde_json::json!([stream, payload])); Ok(()) });
        notifications.sync_done(owner);
        assert_eq!(serde_json::json!(*messages.lock().unwrap()), fixture["records"][format!("sync_{owner}_success")]);
        assert_eq!(fixture["records"][format!("sync_{owner}_success")], fixture["records"][format!("sync_{owner}_failure")]);
        messages.lock().unwrap().clear();
        notifications.ledger_done(owner);
        assert_eq!(serde_json::json!(*messages.lock().unwrap()), fixture["records"][format!("ledger_{owner}")]);
    }
}

#[test]
fn completion_scheduler_is_connected_to_the_served_apps_hub() {
    let main = include_str!("../src/main.rs");
    assert!(main.contains("scheduler.with_notifications(web.job_notifications())"));
    let scheduler = include_str!("../src/jobs/mod.rs");
    assert!(scheduler.contains("self.db = self.db.with_notifications(notifications)"));
}

