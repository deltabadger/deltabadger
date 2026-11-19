-- #710 slice 1 (PLAN-710 §3): several accounts per exchange. exchange_accounts is rebuilt through a hop table, every row
-- keeping its id: UNIQUE (user_id, exchange_id) goes; `number` (1, 2, … per user and exchange, never reused) and
-- `removed_at` (a tombstone) come. Each existing row is number 1.
CREATE TABLE exchange_accounts_710 (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, exchange_id INTEGER NOT NULL, fingerprint TEXT,
  account_id_enc BLOB, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
INSERT INTO exchange_accounts_710 (id, user_id, exchange_id, fingerprint, account_id_enc, created_at, updated_at)
  SELECT id, user_id, exchange_id, fingerprint, account_id_enc, created_at, updated_at FROM exchange_accounts;
DROP TABLE exchange_accounts;
CREATE TABLE exchange_accounts (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  number INTEGER NOT NULL,
  fingerprint TEXT,
  account_id_enc BLOB,
  removed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (user_id, exchange_id, number)
);
INSERT INTO exchange_accounts (id, user_id, exchange_id, number, fingerprint, account_id_enc, created_at, updated_at)
  SELECT id, user_id, exchange_id, 1, fingerprint, account_id_enc, created_at, updated_at FROM exchange_accounts_710;
DROP TABLE exchange_accounts_710;

-- Which account a key or a bot belongs to. No row: the number-1 account of its (user, exchange).
CREATE TABLE exchange_account_keys (api_key_id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL);
CREATE TABLE exchange_account_bots (bot_id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL);

-- Rows are eager: number 1 for every (user, exchange) a table names and no account has yet (identity::FILL, the same
-- statement, runs again after 2.x owned the install).
INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT user_id, exchange_id, 1, datetime('now'), datetime('now') FROM (
    SELECT user_id, exchange_id FROM api_keys
    UNION SELECT user_id, exchange_id FROM bots
    UNION SELECT b.user_id, t.exchange_id FROM transactions t JOIN bots b ON b.id = t.bot_id
    UNION SELECT user_id, exchange_id FROM rules
    UNION SELECT user_id, exchange_id FROM account_transactions
    UNION SELECT user_id, exchange_id FROM account_balances
    UNION SELECT user_id, exchange_id FROM portfolio_venue_snapshots
    UNION SELECT user_id, exchange_id FROM exchange_account_imports
    UNION SELECT user_id, exchange_id FROM import_rows
    UNION SELECT user_id, exchange_id FROM import_cutoffs
    UNION SELECT user_id, exchange_id FROM kucoin_coverage
    UNION SELECT user_id, exchange_id FROM kucoin_withheld_fills
    UNION SELECT o.user_id, s.exchange_id FROM rust_api_order_standalone s JOIN rust_api_orders o ON o.id = s.order_id
  ) n WHERE user_id IS NOT NULL AND exchange_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = n.user_id AND a.exchange_id = n.exchange_id)
  ORDER BY user_id, exchange_id;
