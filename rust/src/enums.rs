//! Rails enum integers, as stored. Pinned by tests/enums.rs and test/contracts/rust_enums_test.rb.
macro_rules! rails_enum {
    ($name:ident { $($variant:ident = $value:literal => $label:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(i64)]
        pub enum $name { $($variant = $value),+ }
        impl $name {
            pub const ALL: &'static [(&'static str, Self)] = &[$(($label, Self::$variant)),+];
            pub fn from_i64(v: i64) -> Option<Self> { Self::ALL.iter().find(|(_, e)| *e as i64 == v).map(|(_, e)| *e) }
            pub fn label(self) -> &'static str { match self { $(Self::$variant => $label),+ } }
        }
    };
}

rails_enum!(BotStatus { Created = 0 => "created", Scheduled = 1 => "scheduled", Stopped = 2 => "stopped", Deleted = 3 => "deleted",
    Executing = 4 => "executing", Retrying = 5 => "retrying", Waiting = 6 => "waiting", Archived = 7 => "archived" });
rails_enum!(RuleStatus { Created = 0 => "created", Scheduled = 1 => "scheduled", Stopped = 2 => "stopped", Deleted = 3 => "deleted", Executing = 4 => "executing", Retrying = 5 => "retrying", Waiting = 6 => "waiting", Archived = 7 => "archived" });
rails_enum!(TxStatus { Submitted = 0 => "submitted", Failed = 1 => "failed", Skipped = 2 => "skipped" });
rails_enum!(TxSide { Buy = 0 => "buy", Sell = 1 => "sell" });
rails_enum!(TxOrderType { MarketOrder = 0 => "market_order", LimitOrder = 1 => "limit_order" });
rails_enum!(TxExternalStatus { Unknown = 0 => "unknown", Open = 1 => "open", Closed = 2 => "closed", Cancelled = 3 => "cancelled", Abandoned = 4 => "abandoned" });
rails_enum!(ApiKeyStatus { PendingValidation = 0 => "pending_validation", Correct = 1 => "correct", Incorrect = 2 => "incorrect", PendingActivation = 3 => "pending_activation" });
rails_enum!(ApiKeyType { Trading = 0 => "trading", Withdrawal = 1 => "withdrawal", ReadOnly = 2 => "read_only" });
rails_enum!(OtpModule { Disabled = 0 => "disabled", Enabled = 1 => "enabled" });

/// `Automation::Statusable.working`.
pub const BOT_WORKING: [BotStatus; 4] = [BotStatus::Scheduled, BotStatus::Executing, BotStatus::Retrying, BotStatus::Waiting];
