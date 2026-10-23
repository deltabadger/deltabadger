-- KuCoin account-history import (plan KI): each export row as stored, keyed by an id of the row itself, so a re-upload,
-- a superset or a complementary export reconciles row by row. No account id: the export's UID is recorded only as an
-- identity observation (exchange_account_imports). The ledger entries are derived from these rows, one UTC second at a time.
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
  UNIQUE (user_id, exchange_id, row_id)
);
CREATE INDEX index_import_rows_on_user_exchange_time ON import_rows (user_id, exchange_id, transacted_at);
