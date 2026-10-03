//! What the engine tells other modules, without calling into them. Each event is sent after the write it
//! reports has committed. Delivery is in memory and at most once: a crash or stop before a consumer runs loses it. The
//! nightly ledger sync covers a lost OrderRecorded. A lost FundsLow only delays the mail: the funds marker is written with
//! the stamp, and the mail service finds it on its next poll.
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// A `transactions` row this engine inserted (submitted, failed or skipped) or recovered: what Rails' Transaction
    /// after_create_commit hears, which enqueues AccountTransaction::SyncJob for the bot's trading key
    /// (app/models/transaction.rb:17-21).
    OrderRecorded { bot_id: i64, transaction_id: i64 },
    /// The engine stamped `bots.last_end_of_funds_notification` where Rails sends BotAlertsMailer#end_of_funds and left the
    /// funds mail marker with it: the daily budget per (user, quote asset) was free and is now spent (Bot::Fundable,
    /// Bot::Failable#record_failure!). The mail service's wake.
    FundsLow { bot_id: i64, user_id: i64, quote_asset_id: Option<i64> },
}

/// The engine's subscribers. Unbounded: nothing is dropped while the process lives.
#[derive(Default)]
pub struct EngineEvents(Vec<UnboundedSender<EngineEvent>>);

impl EngineEvents {
    pub fn subscribe(&mut self) -> UnboundedReceiver<EngineEvent> {
        let (tx, rx) = unbounded_channel();
        self.0.push(tx);
        rx
    }

    pub fn send(&self, e: EngineEvent) {
        for s in &self.0 { let _ = s.send(e.clone()); } // a dropped subscriber is not the engine's concern
    }
}
