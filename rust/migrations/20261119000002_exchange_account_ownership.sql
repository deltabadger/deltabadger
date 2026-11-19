-- #710 slice 3 (PLAN-710 §3, §4.6): what a bot, an order, a withdrawal rule and a standalone order belong to is an account.

-- Withdrawal rules move to a Rust-owned store with their account: 2.x's unique (user_id, type, exchange_id, asset_id) could
-- not hold two accounts' rules for one asset. Each keeps its rules.id (rule_logs, evaluations, acks and `rule:<id>` owners
-- key on it). 2.x's `rules` keeps a row for every id (rule_logs' foreign key) and, for a number-1 rule, the same row
-- (engine::rules::mirror), so 2.x, an export and a handback read what Rust runs. Each existing rule is its number 1's.
CREATE TABLE exchange_account_rules (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  type TEXT NOT NULL,
  status INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  asset_id INTEGER,
  address TEXT,
  settings TEXT NOT NULL,
  settings_changed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (account_id, type, asset_id)
);
-- Every withdrawal rule that names an exchange is copied: its (user, exchange) gets its number 1 first where it has none (as
-- identity::FILL makes them), and a rule whose account still could not be found fails the migration on account_id's NOT
-- NULL rather than going missing. One with no exchange stays in 2.x's table only, as before: no build could run it (every
-- reader joined its exchange), and the mirror never writes its row.
INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT DISTINCT r.user_id, r.exchange_id, 1, datetime('now'), datetime('now') FROM rules r
  WHERE r.type = 'Rules::Withdrawal' AND r.exchange_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = r.user_id AND a.exchange_id = r.exchange_id)
  ORDER BY r.user_id, r.exchange_id;
INSERT INTO exchange_account_rules (id, account_id, user_id, type, status, exchange_id, asset_id, address, settings, settings_changed_at, created_at, updated_at)
  SELECT r.id, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = r.user_id AND a.exchange_id = r.exchange_id AND a.number = 1),
         r.user_id, r.type, r.status, r.exchange_id, r.asset_id, r.address, r.settings, r.settings_changed_at, r.created_at, r.updated_at
  FROM rules r WHERE r.type = 'Rules::Withdrawal' AND r.exchange_id IS NOT NULL ORDER BY r.id;

-- Which account a bot's order belongs to (PLAN-710-4 §2's shape: slice 4 adds its trigger and the relocated venue id).
-- No row: the number-1 account of the order's (user, exchange). Written at placement, in the order row's transaction.
CREATE TABLE exchange_account_transactions (
  transaction_id INTEGER PRIMARY KEY REFERENCES transactions(id) ON DELETE CASCADE,
  account_id INTEGER NOT NULL,
  venue_external_id TEXT,
  UNIQUE (account_id, venue_external_id));

-- rust_withdrawal_ids by account (a venue id is one withdrawal per account: two accounts of one exchange may each have
-- made it), rebuilt through a hop table, every row keeping its rowid. A rule's id is its rule's account's; one of a rule
-- 2.x deleted, its (user, exchange)'s number 1. One whose account cannot be told (a recorder's: the row names no user)
-- keeps NULL, scoped by its exchange: it is unique per exchange, and blocks its venue id for every account of that
-- exchange (engine::withdrawal::id_taken).
CREATE TABLE rust_withdrawal_ids_710 (r INTEGER PRIMARY KEY, account_id INTEGER, exchange_id INTEGER NOT NULL, venue_id TEXT NOT NULL,
  owner TEXT NOT NULL, settled_at TEXT NOT NULL, source TEXT NOT NULL);
INSERT INTO rust_withdrawal_ids_710 (r, account_id, exchange_id, venue_id, owner, settled_at, source)
  SELECT w.rowid, coalesce(
      (SELECT s.account_id FROM exchange_account_rules s WHERE 'rule:' || s.id = w.owner),
      (SELECT a.id FROM rules x JOIN exchange_accounts a ON a.user_id = x.user_id AND a.exchange_id = w.exchange_id AND a.number = 1 WHERE 'rule:' || x.id = w.owner)),
    w.exchange_id, w.venue_id, w.owner, w.settled_at, w.source FROM rust_withdrawal_ids w;
DROP TABLE rust_withdrawal_ids;
CREATE TABLE rust_withdrawal_ids (
  account_id INTEGER,
  exchange_id INTEGER NOT NULL,
  venue_id TEXT NOT NULL,
  owner TEXT NOT NULL,
  settled_at TEXT NOT NULL,
  source TEXT NOT NULL CHECK(source IN ('rust','v2','operator'))
);
CREATE UNIQUE INDEX index_rust_withdrawal_ids_identity ON rust_withdrawal_ids (coalesce(account_id, -exchange_id), venue_id);
INSERT INTO rust_withdrawal_ids (rowid, account_id, exchange_id, venue_id, owner, settled_at, source)
  SELECT r, account_id, exchange_id, venue_id, owner, settled_at, source FROM rust_withdrawal_ids_710;
DROP TABLE rust_withdrawal_ids_710;

-- A standalone order's account (its key, its cancel by venue id), rebuilt through a hop table, every row keeping its order
-- id: each existing one is its (user, exchange)'s number 1. `exchange_id` stays (identity::FILL and the log read it).
CREATE TABLE rust_api_order_standalone_710 (order_id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, exchange_id INTEGER NOT NULL, ticker_id INTEGER NOT NULL,
  order_type TEXT NOT NULL, price TEXT, surface TEXT NOT NULL, identity TEXT, order_snapshot TEXT, external_id TEXT);
INSERT INTO rust_api_order_standalone_710 (order_id, account_id, exchange_id, ticker_id, order_type, price, surface, identity, order_snapshot, external_id)
  SELECT s.order_id, coalesce((SELECT a.id FROM rust_api_orders o JOIN exchange_accounts a ON a.user_id = o.user_id AND a.exchange_id = s.exchange_id AND a.number = 1
                               WHERE o.id = s.order_id), 0),
         s.exchange_id, s.ticker_id, s.order_type, s.price, s.surface, s.identity, s.order_snapshot, s.external_id FROM rust_api_order_standalone s;
DROP TABLE rust_api_order_standalone;
CREATE TABLE rust_api_order_standalone (
  order_id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  ticker_id INTEGER NOT NULL,
  order_type TEXT NOT NULL,
  price TEXT,
  surface TEXT NOT NULL,
  identity TEXT,
  order_snapshot TEXT,
  external_id TEXT
);
CREATE INDEX index_rust_api_order_standalone_on_log ON rust_api_order_standalone (account_id, external_id);
INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, price, surface, identity, order_snapshot, external_id)
  SELECT order_id, account_id, exchange_id, ticker_id, order_type, price, surface, identity, order_snapshot, external_id FROM rust_api_order_standalone_710;
DROP TABLE rust_api_order_standalone_710;
