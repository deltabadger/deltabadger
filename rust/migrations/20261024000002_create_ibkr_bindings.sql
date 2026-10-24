-- IBKR key bindings (Ibkr plan I2, D-IBKR-ACCOUNT): the one brokerage account an IBKR key names, set by its first correct
-- check and never changed. `account_ref` namespaces what is recorded per account (an HMAC prefix, never the id itself);
-- the id is kept only encrypted. Deleting the key deletes its binding (a reconnect binds again).
CREATE TABLE ibkr_bindings (
  api_key_id INTEGER PRIMARY KEY,
  account_ref TEXT NOT NULL,
  account_fingerprint TEXT NOT NULL,
  account_id_enc TEXT NOT NULL,
  bound_at TEXT NOT NULL
);
CREATE TRIGGER ibkr_bindings_no_update BEFORE UPDATE ON ibkr_bindings BEGIN SELECT RAISE(ABORT, 'ibkr_bindings is never updated'); END;
-- IBKR bot ownership (I2): the account an IBKR bot belongs to, set once (at creation when its key is bound, else at its
-- first successful Start) and never changed.
CREATE TABLE ibkr_bot_accounts (
  bot_id INTEGER PRIMARY KEY,
  account_ref TEXT NOT NULL,
  account_id_enc TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TRIGGER ibkr_bot_accounts_no_update BEFORE UPDATE ON ibkr_bot_accounts BEGIN SELECT RAISE(ABORT, 'ibkr_bot_accounts is never updated'); END;
