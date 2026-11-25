-- #710 slice 7b: a second IBKR account. Rust-owned; 2.x never reads these.
-- A connection IBKR has not named an account for yet (D-IBKR-REGISTRATION): its generated keys and pasted credentials,
-- encrypted as api_keys keeps them, out of api_keys until a check names the account (then it becomes that account's key).
-- `target_account_id`: the account it was started from (NULL: a venue tile). `revision` moves with every write, so a
-- stale form is refused. `status` 0 (keys made) or 3 (awaiting IBKR's activation).
-- `baseline`: every slot of its exchange as the submit that set status 3 found it; a finish into a slot replaced since closes it.
CREATE TABLE ibkr_registrations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  target_account_id INTEGER,
  revision INTEGER NOT NULL,
  rsa_signature_key TEXT NOT NULL,
  rsa_encryption_key TEXT NOT NULL,
  dh_param TEXT NOT NULL,
  key TEXT,
  access_token TEXT,
  secret TEXT,
  ibkr_realm TEXT,
  status INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  baseline TEXT
);
-- The IBKR accounts an exchange account holds history of (its keys' bindings, its bots' accounts), by fingerprint;
-- `unconfirmed-after-2.x` (source `takeover`): history 2.x may have synced there, of an account nobody knows.
CREATE TABLE ibkr_account_evidence (
  account_id INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  source TEXT NOT NULL,
  PRIMARY KEY (account_id, fingerprint)
);
-- Every /iserver/accounts read of a login (an HMAC of its full material), in order: `seq` the latest taken, the verdict
-- of `committed_seq` the one acted on, naming `committed_fingerprint`. Reads are serialized by the login's lock.
CREATE TABLE ibkr_login_reads (
  login_digest TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  committed_seq INTEGER NOT NULL,
  committed_fingerprint TEXT
);
