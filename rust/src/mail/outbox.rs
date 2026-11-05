//! D-DESKTOP-NOTIFY (PLAN-B6-3 §3, r4 §3.1): the desktop's bot alerts, waiting for the launcher to show them.
//!
//! The mail sender offers a rendered notice here and goes on with its other markers; the launcher lists the notices
//! (`GET /desktop/notices`), shows each one as a system notification, and acks it (`POST /desktop/notices/ack`). The
//! sender then `take`s the ack and clears the marker. An entry not acked by its deadline is gone: the sender counts that
//! as a failed delivery and offers it again on its retry schedule. Ack against deadline is settled under the one lock,
//! so an ack that comes after the deadline is ignored. Nothing here is written to disk: the markers on the bots' rows
//! are the queue, and an engine restart offers them again.
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use tokio::sync::Notify;
use tokio::time::Instant;

/// The most notices waiting at once. A full outbox refuses an offer; it never drops one it holds.
pub const CAP: usize = 50;
/// The most notices one `GET /desktop/notices` returns.
pub const LIST_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    /// engine::notice::Pending::id: bot, marker, error kind, stamped_at.
    pub id: String,
    pub bot_id: i64,
    pub title: String,
    pub body: String,
}

/// What became of an offered notice, as the sender takes it.
#[derive(Debug, PartialEq, Eq)]
pub enum Taken {
    /// The launcher acked it in time; it is no longer held.
    Acked,
    /// Still waiting for the launcher.
    Waiting,
    /// Its deadline passed unacked (or it was never offered): no longer held.
    Gone,
}

struct Entry {
    notice: Notice,
    deadline: Instant,
    acked: bool,
}

#[derive(Default)]
pub struct Outbox {
    entries: Mutex<VecDeque<Entry>>,
    acked: Notify,
}

impl Outbox {
    fn entries(&self) -> std::sync::MutexGuard<'_, VecDeque<Entry>> { self.entries.lock().unwrap_or_else(PoisonError::into_inner) }

    /// Unacked entries past their deadline go; an acked one waits for the sender's `take` (an hour at most, for a
    /// marker that vanished before the sender came back to it).
    fn prune(entries: &mut VecDeque<Entry>, now: Instant) {
        entries.retain(|e| if e.acked { e.deadline + std::time::Duration::from_secs(3600) > now } else { e.deadline > now });
    }

    /// Holds `notice` until `deadline`. False when the outbox is full. An id already held keeps its entry.
    pub fn offer(&self, notice: Notice, deadline: Instant) -> bool {
        let mut entries = self.entries();
        Self::prune(&mut entries, Instant::now());
        if entries.iter().any(|e| e.notice.id == notice.id) { return true; }
        if entries.len() >= CAP { return false; }
        entries.push_back(Entry { notice, deadline, acked: false });
        true
    }

    /// The unacked notices, oldest first, at most `limit`.
    pub fn list(&self, limit: usize) -> Vec<Notice> {
        let mut entries = self.entries();
        Self::prune(&mut entries, Instant::now());
        entries.iter().filter(|e| !e.acked).take(limit).map(|e| e.notice.clone()).collect()
    }

    /// The launcher showed these. Unknown ids, and ids past their deadline, are ignored. Wakes the sender.
    pub fn ack(&self, ids: &[String]) {
        let now = Instant::now();
        let mut entries = self.entries();
        for e in entries.iter_mut().filter(|e| e.deadline > now && ids.contains(&e.notice.id)) { e.acked = true; }
        drop(entries);
        self.acked.notify_one();
    }

    /// The sender's look at one notice: Acked and Gone release it.
    pub fn take(&self, id: &str) -> Taken {
        let now = Instant::now();
        let mut entries = self.entries();
        let Some(i) = entries.iter().position(|e| e.notice.id == id) else { return Taken::Gone };
        let e = &entries[i];
        if e.acked { entries.remove(i); return Taken::Acked; }
        if e.deadline > now { return Taken::Waiting; }
        entries.remove(i);
        Taken::Gone
    }

    /// A notice whose marker is gone (expired, cleared).
    pub fn remove(&self, id: &str) { self.entries().retain(|e| e.notice.id != id); }

    /// Resolves after the next `ack`.
    pub async fn acked(&self) { self.acked.notified().await }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn notice(id: &str) -> Notice { Notice { id: id.into(), bot_id: 1, title: "t".into(), body: "b".into() } }

    #[tokio::test]
    async fn an_ack_in_time_is_taken_once_and_one_after_the_deadline_is_ignored() {
        let o = Outbox::default();
        let soon = Instant::now() + Duration::from_millis(50);
        assert!(o.offer(notice("a"), soon) && o.offer(notice("b"), soon));
        assert_eq!(o.take("a"), Taken::Waiting);
        o.ack(&["a".into(), "unknown".into()]);
        assert_eq!(o.list(10), vec![notice("b")], "an acked notice is not listed again");
        assert_eq!(o.take("a"), Taken::Acked);
        assert_eq!(o.take("a"), Taken::Gone);
        tokio::time::sleep(Duration::from_millis(60)).await;
        o.ack(&["b".into()]);
        assert_eq!(o.take("b"), Taken::Gone, "too late: the sender counts it as a failure");
    }

    #[tokio::test]
    async fn a_full_outbox_refuses_without_dropping_and_an_id_is_held_once() {
        let o = Outbox::default();
        let later = Instant::now() + Duration::from_secs(60);
        for i in 0..CAP { assert!(o.offer(notice(&i.to_string()), later)); }
        assert!(o.offer(notice("0"), later), "already held");
        assert!(!o.offer(notice("new"), later));
        let listed = o.list(usize::MAX);
        assert_eq!((listed.len(), listed[0].id.as_str()), (CAP, "0"), "oldest first");
        assert_eq!(o.list(LIST_LIMIT).len(), LIST_LIMIT);
    }
}
