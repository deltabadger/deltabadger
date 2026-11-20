-- #710 slice 5 (PLAN-710-5 §5.7): KuCoin HF coverage (the marker, exports, certified and attempted units, missing pairs,
-- ambiguous seconds) is its account's: rebuilt through a hop table as (id, account_id, …), every row keeping its id and its
-- content on its (user, exchange)'s number 1, which is made first where none is there.
INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT DISTINCT k.user_id, k.exchange_id, 1, datetime('now'), datetime('now') FROM kucoin_coverage k
  WHERE NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = k.user_id AND a.exchange_id = k.exchange_id AND a.number = 1)
  ORDER BY k.user_id, k.exchange_id;
CREATE TABLE kucoin_coverage_710_5 (id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, source TEXT NOT NULL, start_at TEXT NOT NULL,
  end_at TEXT NOT NULL, updated_at TEXT NOT NULL);
INSERT INTO kucoin_coverage_710_5 (id, account_id, source, start_at, end_at, updated_at)
  SELECT k.id, (SELECT a.id FROM exchange_accounts a WHERE a.user_id = k.user_id AND a.exchange_id = k.exchange_id AND a.number = 1),
         k.source, k.start_at, k.end_at, k.updated_at FROM kucoin_coverage k;
DROP TABLE kucoin_coverage;
CREATE TABLE kucoin_coverage (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL,
  source TEXT NOT NULL,
  start_at TEXT NOT NULL,
  end_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX index_kucoin_coverage_on_account_source ON kucoin_coverage (account_id, source, start_at);
INSERT INTO kucoin_coverage (id, account_id, source, start_at, end_at, updated_at)
  SELECT id, account_id, source, start_at, end_at, updated_at FROM kucoin_coverage_710_5;
DROP TABLE kucoin_coverage_710_5;
