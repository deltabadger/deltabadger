-- E0: outcome provenance and operator-only recovery candidates; Rails tables are unchanged.
CREATE TABLE rust_rule_log_origins (
  rule_log_id INTEGER PRIMARY KEY,
  owner TEXT NOT NULL,
  client_id TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE rust_withdrawal_candidates (
  owner TEXT NOT NULL PRIMARY KEY,
  client_id TEXT NOT NULL,
  candidates TEXT NOT NULL
);
