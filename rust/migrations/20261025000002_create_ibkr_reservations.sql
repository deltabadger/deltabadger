-- IBKR sell reservations (Ibkr plan I4b, D-IBKR-SELLCAP): a margin account can short, so a sell is capped by the position
-- less what this install's own unsettled sells of the same asset still hold. One row per sell intent, written in the
-- intent's transaction; `transaction_id` set when its order row is written; released once that row is closed or cancelled
-- (or the intent is gone without a row: nothing was placed). An abandoned order keeps its reservation. Keyed by account_ref.
CREATE TABLE ibkr_reservations (
  id INTEGER PRIMARY KEY,
  api_key_id INTEGER NOT NULL,
  account_ref TEXT NOT NULL,
  conid INTEGER NOT NULL,
  bot_id INTEGER NOT NULL,
  cl_ord_id TEXT NOT NULL,
  transaction_id INTEGER,
  quantity TEXT NOT NULL,
  created_at TEXT NOT NULL,
  released_at TEXT
);
CREATE UNIQUE INDEX index_ibkr_reservations_on_cl_ord_id ON ibkr_reservations (cl_ord_id);
