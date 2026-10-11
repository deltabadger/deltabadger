//! What the engine tells other modules, without calling into them. Each event is sent after the write it
//! reports has committed. Delivery is in memory and at most once: a crash or stop before a consumer runs loses it. The
//! nightly ledger sync covers a lost OrderRecorded. A lost FundsLow only delays the mail: the funds marker is written with
//! the stamp, and the mail service finds it on its next poll.
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// First own tick, all own rows skipped: Rails modal notification, after row commits.
    BelowMinimum { bot_id: i64, transaction_ids: Vec<i64> },
    /// A `transactions` row this engine inserted (submitted, failed or skipped) or recovered: what Rails' Transaction
    /// after_create_commit hears, which enqueues AccountTransaction::SyncJob for the bot's trading key
    /// (app/models/transaction.rb:17-21).
    OrderRecorded { bot_id: i64, transaction_id: i64 },
    /// A tick or a follow-up poll of this bot ran, and may have written its orders (a fill, a cancel, an order found
    /// abandoned): what Rails' Transaction after_update_commit hears, which broadcasts the row and, through
    /// Bot::UpdateMetricsJob, the bot's figures (app/models/transaction.rb:22-25). Sent after every tick and every poll,
    /// on success and on error, with nothing read to decide: a spurious one costs a coalesced publication, and none can
    /// be lost. Only the figures service hears it.
    OrderUpdated { bot_id: i64 },
    /// The engine stamped `bots.last_end_of_funds_notification` where Rails sends BotAlertsMailer#end_of_funds and left the
    /// funds mail marker with it: the daily budget per (user, quote asset) was free and is now spent (Bot::Fundable,
    /// Bot::Failable#record_failure!). The mail service's wake.
    FundsLow { bot_id: i64, user_id: i64, quote_asset_id: Option<i64> },
}

impl EngineEvent {
    pub fn is_warning(&self) -> bool { matches!(self, Self::BelowMinimum { .. }) }
    /// The kinds a subscriber hears unless it asks for others: all but `OrderUpdated`, which only the figures service
    /// wants.
    pub fn is_classic(&self) -> bool {
        !matches!(self, EngineEvent::OrderUpdated { .. } | EngineEvent::BelowMinimum { .. })
    }

    /// An order inserted or written: what the figures service hears.
    pub fn is_order(&self) -> bool {
        matches!(self, EngineEvent::OrderRecorded { .. } | EngineEvent::OrderUpdated { .. })
    }
}

/// The engine's subscribers, each with the kinds it hears. Unbounded: nothing it asked for is dropped while the process
/// lives, and nothing it did not ask for is queued.
#[derive(Default)]
pub struct EngineEvents(Vec<(Interest, UnboundedSender<EngineEvent>)>);

/// The kinds of event a subscriber hears.
pub type Interest = fn(&EngineEvent) -> bool;

impl EngineEvents {
    /// Every event but `OrderUpdated` (`EngineEvent::is_classic`).
    pub fn subscribe(&mut self) -> UnboundedReceiver<EngineEvent> {
        self.subscribe_to(EngineEvent::is_classic)
    }

    /// The events `wants` accepts.
    pub fn subscribe_to(&mut self, wants: Interest) -> UnboundedReceiver<EngineEvent> {
        let (tx, rx) = unbounded_channel();
        self.0.push((wants, tx));
        rx
    }

    pub fn send(&self, e: EngineEvent) {
        for (wants, s) in &self.0 {
            if wants(&e) { let _ = s.send(e.clone()); } // allow-swallow: a dropped subscriber is not the engine's concern
        }
    }
}
