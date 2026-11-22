-- #710 slice 6 (PLAN-710-6 §5.1): an account's standing as a duplicate of a lower one of its exchange (its survivor),
-- stored and sticky. identity::reconsider is its one writer: it enters a row when every signal agrees, removes one on a
-- positive contradiction, and every key event of either account deletes it. `basis`: both accounts' live keys and a
-- digest of each one's stored credential at entry (Merge requires it unchanged). Rust-only: 2.x never reads it.
CREATE TABLE exchange_account_standings (
  duplicate_id INTEGER PRIMARY KEY,
  survivor_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  since TEXT NOT NULL,
  basis TEXT NOT NULL
);
CREATE INDEX index_exchange_account_standings_on_user_exchange ON exchange_account_standings (user_id, exchange_id);
