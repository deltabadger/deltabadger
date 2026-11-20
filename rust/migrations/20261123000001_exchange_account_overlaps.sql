-- #710 slice 6 (PLAN-710-6 §5.1): an account 2+ whose earliest trade rows share venue-native trade ids and their facts with
-- a lower live account of its exchange is the same venue account (`identity::observe_overlap`). One observation per
-- account; the duplicate standing itself is derived when read (`identity::duplicates`).
CREATE TABLE exchange_account_overlaps (
  account_id INTEGER PRIMARY KEY,
  other_account_id INTEGER NOT NULL,
  matches INTEGER NOT NULL,
  observed_at TEXT NOT NULL
);
