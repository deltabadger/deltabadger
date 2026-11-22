-- #710 slice 7a §9.7 rule R8: which held API event claims a row of another account's copy of this account's export. A row is
-- the copy of one held event at most (a KuCoin second's fills, summed, claim together), across pages and syncs; the claim
-- is the account's whose API sync held the event. Rust-only: 2.x never reads it.
CREATE TABLE exchange_account_hold_claims (
  transaction_id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL,
  claimant TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX index_exchange_account_hold_claims_on_account ON exchange_account_hold_claims (account_id);
