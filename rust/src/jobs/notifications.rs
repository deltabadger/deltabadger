//! Best-effort completion delivery, called only after job write units have ended.
use std::sync::Arc;

type Publish = Arc<dyn Fn(&str, &str) -> Result<(), ()> + std::marker::Send + Sync>;
type Log = Arc<dyn Fn(&str) + std::marker::Send + Sync>;
type Balances = Arc<dyn Fn(i64) + std::marker::Send + Sync>;

#[derive(Clone)]
pub struct Notifications {
    send: Publish,
    log: Log,
    balances: Balances,
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
            balances: Arc::new(|_| {}),
        }
    }
    /// What a key's balances changing tells (D-LOW-FUNDS-STATUS: its owner's bots on that venue redraw their status bars).
    pub fn with_balances(mut self, changed: impl Fn(i64) + std::marker::Send + Sync + 'static) -> Self {
        self.balances = Arc::new(changed);
        self
    }
    /// A key's synced balances became current (a balance sync's success committed) or stopped being so (its job was
    /// marked incomplete).
    pub fn balances_changed(&self, key_id: i64) {
        (self.balances)(key_id);
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
    /// Setup::SeedAndSyncJob#broadcast_sync_completed: the "syncing" banner goes.
    pub fn setup_synced(&self) {
        if (self.send)("settings_sync", "<turbo-stream action=\"remove\" target=\"flash-syncing\"></turbo-stream>").is_err() {
            (self.log)("[setup] completion broadcast failed");
        }
    }
    /// A tax report request's panel (`broadcast_replace_to "user_<id>", :tax_report`).
    pub fn tax_report(&self, owner: i64, payload: &str) {
        if (self.send)(&format!("user_{owner}:tax_report"), payload).is_err() {
            (self.log)("[tax] report broadcast failed");
        }
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
            balances: Arc::new(|_| {}),
        };
        notifications.sync_done(42);
        notifications.ledger_done(42);
        assert_eq!(
            *logs.lock().unwrap(),
            vec!["[tracker] completion broadcast failed"; 2]
        );
    }
}
