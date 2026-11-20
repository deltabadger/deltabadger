-- #710 slice 5 (PLAN-710-5 §4): balances and venue snapshots by account. 2.x's unique indexes name the exchange, so both
-- move to Rust-owned stores keyed by account_id; 2.x's `account_balances` and `portfolio_venue_snapshots` stay, unread.
-- Each store keeps 2.x's columns in 2.x's order with 2.x's declared types (so the stored values and their affinity are
-- 2.x's) and adds account_id last. Every row is copied with its id onto its (user, exchange)'s number 1, made first where
-- none is there (as the slice 3 and 4 migrations do); each store's sequence starts at its 2.x table's, so a new row gets
-- the id 2.x would have given it.
INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at)
  SELECT user_id, exchange_id, 1, datetime('now'), datetime('now') FROM (
    SELECT user_id, exchange_id FROM account_balances UNION SELECT user_id, exchange_id FROM portfolio_venue_snapshots) n
  WHERE NOT EXISTS (SELECT 1 FROM exchange_accounts a WHERE a.user_id = n.user_id AND a.exchange_id = n.exchange_id AND a.number = 1)
  ORDER BY user_id, exchange_id;

CREATE TABLE exchange_account_balances (
  id integer PRIMARY KEY AUTOINCREMENT NOT NULL,
  asset_id integer NOT NULL,
  created_at datetime(6) NOT NULL,
  exchange_id integer NOT NULL,
  free decimal(32,16) DEFAULT 0.0 NOT NULL,
  locked decimal(32,16) DEFAULT 0.0 NOT NULL,
  priced_at datetime(6),
  synced_at datetime(6) NOT NULL,
  updated_at datetime(6) NOT NULL,
  usd_price decimal(20,8),
  usd_value decimal(20,8),
  user_id integer NOT NULL,
  account_id INTEGER NOT NULL,
  UNIQUE (account_id, asset_id)
);
CREATE INDEX index_exchange_account_balances_on_user_exchange ON exchange_account_balances (user_id, exchange_id);
CREATE INDEX index_exchange_account_balances_on_asset_id ON exchange_account_balances (asset_id);
INSERT INTO exchange_account_balances (id, asset_id, created_at, exchange_id, free, locked, priced_at, synced_at, updated_at, usd_price, usd_value, user_id, account_id)
  SELECT t.id, t.asset_id, t.created_at, t.exchange_id, t.free, t.locked, t.priced_at, t.synced_at, t.updated_at, t.usd_price, t.usd_value, t.user_id, a.id
  FROM account_balances t LEFT JOIN exchange_accounts a ON a.user_id = t.user_id AND a.exchange_id = t.exchange_id AND a.number = 1 ORDER BY t.id;
DELETE FROM sqlite_sequence WHERE name = 'exchange_account_balances';
INSERT INTO sqlite_sequence (name, seq) SELECT 'exchange_account_balances',
  max(coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'account_balances'), 0), coalesce((SELECT max(id) FROM exchange_account_balances), 0));

CREATE TABLE exchange_account_snapshots (
  id integer PRIMARY KEY AUTOINCREMENT NOT NULL,
  user_id integer NOT NULL,
  exchange_id integer NOT NULL,
  date date NOT NULL,
  value_usd decimal(20,8) DEFAULT 0.0 NOT NULL,
  invested_usd decimal(20,8) DEFAULT 0.0 NOT NULL,
  held_value_usd decimal(20,8),
  held_cost_usd decimal(20,8),
  partial boolean DEFAULT FALSE NOT NULL,
  created_at datetime(6) NOT NULL,
  updated_at datetime(6) NOT NULL,
  account_id INTEGER NOT NULL,
  UNIQUE (account_id, date)
);
CREATE INDEX index_exchange_account_snapshots_on_user_date ON exchange_account_snapshots (user_id, date);
INSERT INTO exchange_account_snapshots (id, user_id, exchange_id, date, value_usd, invested_usd, held_value_usd, held_cost_usd, partial, created_at, updated_at, account_id)
  SELECT t.id, t.user_id, t.exchange_id, t.date, t.value_usd, t.invested_usd, t.held_value_usd, t.held_cost_usd, t.partial, t.created_at, t.updated_at, a.id
  FROM portfolio_venue_snapshots t LEFT JOIN exchange_accounts a ON a.user_id = t.user_id AND a.exchange_id = t.exchange_id AND a.number = 1 ORDER BY t.id;
DELETE FROM sqlite_sequence WHERE name = 'exchange_account_snapshots';
INSERT INTO sqlite_sequence (name, seq) SELECT 'exchange_account_snapshots',
  max(coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'portfolio_venue_snapshots'), 0), coalesce((SELECT max(id) FROM exchange_account_snapshots), 0));
