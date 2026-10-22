-- Exchange-account identity (plan AF, slice AF1): recorded only, never enforced. No foreign key to a Rails table:
-- Rails deletes api_keys and app_configs with delete_all (EncryptedCredentials::Reset) and must keep doing so.
-- A NULL fingerprint is an unknown account; account_id_enc is the raw account id in the credential envelope.
CREATE TABLE exchange_accounts (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  fingerprint TEXT,
  account_id_enc BLOB,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (user_id, exchange_id)
);
-- The last identity observation of each credential, and its last failed read.
CREATE TABLE exchange_account_credentials (
  api_key_id INTEGER PRIMARY KEY,
  exchange_id INTEGER NOT NULL,
  material_digest TEXT NOT NULL,
  observed_credential_version TEXT,
  fingerprint TEXT,
  account_id_enc BLOB,
  observed_at TEXT,
  last_failure_at TEXT,
  last_failure_code TEXT
);
-- One row per identity-bearing import.
CREATE TABLE exchange_account_imports (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  account_id_enc BLOB NOT NULL,
  observed_at TEXT NOT NULL
);
CREATE INDEX index_exchange_account_imports_on_user_exchange ON exchange_account_imports (user_id, exchange_id);
