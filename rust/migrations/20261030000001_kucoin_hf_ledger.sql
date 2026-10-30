-- KuCoin HF fills in the ledger (D-KUCOIN-HF-LEDGER). Rust-owned, so an app_configs wipe or a Rails reset keeps them.
-- kucoin_coverage: per (user, exchange), the marker `hf_enabled`, each export's period, certified HF units (`hf_api`),
-- every HF unit (`hf_attempt`, `hf_attempt_open` while it runs), absent pairs (`missing:<symbol>`) and the seconds the
-- one-trade rule could not pair (`ambiguous`). Times are whole UTC seconds, [start_at, end_at).
CREATE TABLE kucoin_coverage (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  source TEXT NOT NULL,
  start_at TEXT NOT NULL,
  end_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX index_kucoin_coverage_on_user_exchange_source ON kucoin_coverage (user_id, exchange_id, source, start_at);
-- API fills at a second the user's restored entries already hold: kept here, never in account_transactions.
CREATE TABLE kucoin_withheld_fills (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  tx_id TEXT NOT NULL,
  transacted_at TEXT NOT NULL,
  entry TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (user_id, exchange_id, tx_id)
);
CREATE INDEX index_kucoin_withheld_fills_on_user_exchange_time ON kucoin_withheld_fills (user_id, exchange_id, transacted_at);
