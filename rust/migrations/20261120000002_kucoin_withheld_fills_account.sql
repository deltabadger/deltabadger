-- #710 slice 4 (PLAN-710-4 §2): a withheld KuCoin HF fill (a ledger fill id the visible history hides behind a restore) is
-- its account's: rebuilt through a hop table as (account_id, tx_id, …), unique per account, every row keeping its id and
-- its content on its (user, exchange)'s number 1, which is made first where none is there.
INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT DISTINCT w.user_id, w.exchange_id, 1, datetime('now'), datetime('now') FROM kucoin_withheld_fills w
  WHERE NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = w.user_id AND a.exchange_id = w.exchange_id AND a.number = 1)
  ORDER BY w.user_id, w.exchange_id;
CREATE TABLE kucoin_withheld_fills_710_4 (id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, tx_id TEXT NOT NULL, transacted_at TEXT NOT NULL,
  entry TEXT NOT NULL, created_at TEXT NOT NULL);
INSERT INTO kucoin_withheld_fills_710_4 (id, account_id, tx_id, transacted_at, entry, created_at)
  SELECT w.id, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = w.user_id AND a.exchange_id = w.exchange_id AND a.number = 1),
         w.tx_id, w.transacted_at, w.entry, w.created_at FROM kucoin_withheld_fills w;
DROP TABLE kucoin_withheld_fills;
CREATE TABLE kucoin_withheld_fills (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL,
  tx_id TEXT NOT NULL,
  transacted_at TEXT NOT NULL,
  entry TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (account_id, tx_id)
);
CREATE INDEX index_kucoin_withheld_fills_on_account_time ON kucoin_withheld_fills (account_id, transacted_at);
INSERT INTO kucoin_withheld_fills (id, account_id, tx_id, transacted_at, entry, created_at)
  SELECT id, account_id, tx_id, transacted_at, entry, created_at FROM kucoin_withheld_fills_710_4;
DROP TABLE kucoin_withheld_fills_710_4;
