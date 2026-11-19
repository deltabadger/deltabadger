-- #710 slice 4 (PLAN-710-4 §2): the ledger's and a bot order's venue id of an account 2+. 2.x's unique (user_id, exchange_id, tx_id) of
-- account_transactions cannot hold two accounts' rows under one id, so a row of an account 2+ stores tx_id NULL there and
-- its venue id here; a number-1 row has no row here and keeps its id in 2.x's column (venue_ids::row_id reads both).
-- account_id has no foreign key: accounts are never deleted (tombstones). The row's own one cascades: the membership goes
-- with its row, and only then.
CREATE TABLE exchange_account_rows (
  account_transaction_id INTEGER PRIMARY KEY REFERENCES account_transactions(id) ON DELETE CASCADE,
  account_id INTEGER NOT NULL,
  venue_tx_id TEXT,
  UNIQUE (account_id, venue_tx_id));
-- A membership names an account 2+ of the row's own user and exchange, and its row's 2.x id is NULL: "no membership"
-- keeps one meaning (number 1), and no id is stored twice.
CREATE TRIGGER exchange_account_rows_member BEFORE INSERT ON exchange_account_rows
  WHEN NOT EXISTS (SELECT 1 FROM exchange_accounts a JOIN account_transactions t ON t.id = NEW.account_transaction_id
    WHERE a.id = NEW.account_id AND a.number >= 2 AND a.user_id = t.user_id AND a.exchange_id = t.exchange_id AND t.tx_id IS NULL)
  BEGIN SELECT RAISE(ABORT, 'a member is an account 2+ of the row''s exchange, and its 2.x id is NULL'); END;
-- The same for a bot's order (exchange_account_transactions, slice 3's table in its final shape): its account is an account
-- 2+ of its bot's user and its own exchange, and 2.x's external_id is NULL (the venue id is venue_external_id).
CREATE TRIGGER exchange_account_transactions_member BEFORE INSERT ON exchange_account_transactions
  WHEN NOT EXISTS (SELECT 1 FROM exchange_accounts a JOIN transactions t ON t.id = NEW.transaction_id JOIN bots b ON b.id = t.bot_id
    WHERE a.id = NEW.account_id AND a.number >= 2 AND a.user_id = b.user_id AND a.exchange_id = t.exchange_id AND t.external_id IS NULL)
  BEGIN SELECT RAISE(ABORT, 'a member is an account 2+ of the order''s exchange, and its 2.x id is NULL'); END;
