//! Best-effort completion delivery, called only after job write units have ended.
use std::sync::Arc;

type Publish = Arc<dyn Fn(&str, &str) -> Result<(), ()> + std::marker::Send + Sync>;
type Log = Arc<dyn Fn(&str) + std::marker::Send + Sync>;

#[derive(Clone)]
pub struct Notifications {
    send: Publish,
    log: Log,
}
impl Default for Notifications {
    fn default() -> Self {
        Self::new(|_, _| Ok(()))
    }
}
impl Notifications {
    pub fn new(
        send: impl Fn(&str, &str) -> Result<(), ()> + std::marker::Send + Sync + 'static,
    ) -> Self {
        Self {
            send: Arc::new(send),
            log: Arc::new(crate::engine::log),
        }
    }
    pub fn sync_done(&self, owner: i64) {
        self.publish(
            owner,
            "<turbo-stream action=\"remove\" target=\"sync-progress\"></turbo-stream>",
        );
    }
    pub fn ledger_done(&self, owner: i64) {
        self.publish(owner, "<turbo-stream action=\"refresh\"></turbo-stream>");
    }
    fn publish(&self, owner: i64, payload: &str) {
        if (self.send)(&format!("user_{owner}:sync"), payload).is_err() {
            (self.log)("[tracker] completion broadcast failed");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_delivery_failure_is_logged_once_without_error_data() {
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = logs.clone();
        let notifications = Notifications {
            send: Arc::new(|_, _| Err(())),
            log: Arc::new(move |s| captured.lock().unwrap().push(s.to_owned())),
        };
        notifications.sync_done(42);
        notifications.ledger_done(42);
        assert_eq!(
            *logs.lock().unwrap(),
            vec!["[tracker] completion broadcast failed"; 2]
        );
    }
}
