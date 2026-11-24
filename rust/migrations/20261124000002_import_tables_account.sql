-- #710 slice 7a (PLAN-710-7 §0.6): KuCoin export rows, the API's takeover cutoffs and the identity observations of
-- imports are their account's: each rebuilt through a hop table with `account_id`, every row keeping its id and its content
-- on its (user, exchange)'s number 1, which is made first where none is there.
INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT DISTINCT k.user_id, k.exchange_id, 1, datetime('now'), datetime('now') FROM (
    SELECT user_id, exchange_id FROM import_rows UNION SELECT user_id, exchange_id FROM import_cutoffs
    UNION SELECT user_id, exchange_id FROM exchange_account_imports) k
  WHERE NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = k.user_id AND a.exchange_id = k.exchange_id AND a.number = 1)
  ORDER BY k.user_id, k.exchange_id;

CREATE TABLE import_rows_710_7 AS SELECT i.*, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = i.user_id AND a.exchange_id = i.exchange_id AND a.number = 1) AS account_id FROM import_rows i;
DROP TABLE import_rows;
CREATE TABLE import_rows (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  row_id TEXT NOT NULL,
  account_type TEXT NOT NULL,
  currency TEXT NOT NULL,
  side TEXT NOT NULL,
  amount TEXT NOT NULL,
  fee TEXT NOT NULL,
  transacted_at TEXT NOT NULL,
  remark TEXT NOT NULL,
  row_type TEXT NOT NULL,
  created_at TEXT NOT NULL,
  account_id INTEGER NOT NULL,
  UNIQUE (account_id, row_id)
);
CREATE INDEX index_import_rows_on_account_time ON import_rows (account_id, transacted_at);
INSERT INTO import_rows (id, user_id, exchange_id, row_id, account_type, currency, side, amount, fee, transacted_at, remark, row_type, created_at, account_id)
  SELECT id, user_id, exchange_id, row_id, account_type, currency, side, amount, fee, transacted_at, remark, row_type, created_at, account_id FROM import_rows_710_7;
DROP TABLE import_rows_710_7;

CREATE TABLE import_cutoffs_710_7 AS SELECT i.*, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = i.user_id AND a.exchange_id = i.exchange_id AND a.number = 1) AS account_id FROM import_cutoffs i;
DROP TABLE import_cutoffs;
CREATE TABLE import_cutoffs (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  class TEXT NOT NULL,
  cutoff_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  account_id INTEGER NOT NULL,
  UNIQUE (account_id, class)
);
INSERT INTO import_cutoffs (id, user_id, exchange_id, class, cutoff_at, updated_at, account_id)
  SELECT id, user_id, exchange_id, class, cutoff_at, updated_at, account_id FROM import_cutoffs_710_7;
DROP TABLE import_cutoffs_710_7;

CREATE TABLE exchange_account_imports_710_7 AS SELECT i.*, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = i.user_id AND a.exchange_id = i.exchange_id AND a.number = 1) AS account_id FROM exchange_account_imports i;
DROP TABLE exchange_account_imports;
CREATE TABLE exchange_account_imports (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  account_id_enc BLOB NOT NULL,
  observed_at TEXT NOT NULL,
  account_id INTEGER NOT NULL
);
CREATE INDEX index_exchange_account_imports_on_user_exchange ON exchange_account_imports (user_id, exchange_id);
CREATE INDEX index_exchange_account_imports_on_account ON exchange_account_imports (account_id);
INSERT INTO exchange_account_imports (id, user_id, exchange_id, fingerprint, account_id_enc, observed_at, account_id)
  SELECT id, user_id, exchange_id, fingerprint, account_id_enc, observed_at, account_id FROM exchange_account_imports_710_7;
DROP TABLE exchange_account_imports_710_7;
