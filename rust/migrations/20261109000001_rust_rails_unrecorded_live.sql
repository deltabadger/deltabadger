-- A28-3 (PLAN-A28-3 r2.3, D-RAILS-UNRECORDED-LIVE-HISTORY): the live order-history read of one rust_rails_unrecorded
-- obligation, kept only when it failed. Rust-only. `stamp`: the trading key's credential version it was read with (a new
-- key starts afresh); `refused_at`: the venue refused that key (never read again with it); `attempts`: other failures,
-- one per pass; `last_at`: the last failure. Written by engine::rails_unrecorded::settle in its own transaction.
CREATE TABLE rust_rails_unrecorded_live (obligation_id INTEGER PRIMARY KEY, stamp TEXT NOT NULL, attempts INTEGER NOT NULL,
  refused_at TEXT, last_at TEXT);
