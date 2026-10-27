-- W2: durable withdrawal evaluations, intents, settlement identities and recovery acknowledgements.
CREATE TABLE rust_rule_evaluations (
  id INTEGER PRIMARY KEY,
  rule_id INTEGER NOT NULL,
  source TEXT NOT NULL,
  created_at TEXT NOT NULL,
  consumed_at TEXT,
  outcome TEXT CHECK(outcome IN ('skipped','logged','intent'))
);
CREATE UNIQUE INDEX index_rust_rule_evaluations_identity ON rust_rule_evaluations (rule_id, source);
CREATE TABLE rust_withdrawal_intents (
  owner TEXT NOT NULL PRIMARY KEY,
  evaluation_id INTEGER,
  state TEXT NOT NULL CHECK(state IN ('prepared','dispatched','unresolved')),
  client_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  api_key_id INTEGER NOT NULL,
  credential_digest TEXT NOT NULL,
  host TEXT NOT NULL,
  account_scope TEXT NOT NULL,
  key_identity TEXT NOT NULL,
  params_enc TEXT NOT NULL,
  fee_basis TEXT NOT NULL,
  budget TEXT NOT NULL,
  free_balance TEXT NOT NULL,
  venue_id TEXT,
  reason TEXT
);
CREATE TABLE rust_withdrawal_ids (
  exchange_id INTEGER NOT NULL,
  venue_id TEXT NOT NULL,
  owner TEXT NOT NULL,
  settled_at TEXT NOT NULL,
  source TEXT NOT NULL CHECK(source IN ('rust','rails','operator'))
);
CREATE UNIQUE INDEX index_rust_withdrawal_ids_identity ON rust_withdrawal_ids (exchange_id, venue_id);
CREATE TABLE rust_rails_attempt_acks (
  rule_log_id INTEGER PRIMARY KEY,
  rule_id INTEGER NOT NULL,
  resolution TEXT NOT NULL,
  venue_id TEXT,
  at TEXT NOT NULL
);
CREATE TABLE rust_rails_job_acks (
  active_job_id TEXT NOT NULL PRIMARY KEY,
  rule_id INTEGER NOT NULL,
  resolution TEXT NOT NULL,
  venue_id TEXT,
  at TEXT NOT NULL
);
CREATE TABLE rust_recorder_runs (
  scenario_id TEXT NOT NULL PRIMARY KEY,
  account_ref TEXT NOT NULL,
  destination_digest TEXT NOT NULL,
  amount TEXT NOT NULL,
  evaluation_id INTEGER NOT NULL,
  outcome TEXT,
  created_at TEXT NOT NULL
);
