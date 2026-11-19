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
INSERT INTO exchange_account_rules (id, account_id, user_id, type, status, exchange_id, asset_id, address, settings, settings_changed_at, created_at, updated_at)
  SELECT r.id, a.id, r.user_id, r.type, r.status, r.exchange_id, r.asset_id, r.address, r.settings, r.settings_changed_at, r.created_at, r.updated_at
  FROM rules r JOIN exchange_accounts a ON a.user_id = r.user_id AND a.exchange_id = r.exchange_id AND a.number = 1
  WHERE r.type = 'Rules::Withdrawal' ORDER BY r.id;
