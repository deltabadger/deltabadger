-- #710 (PLAN-710-confirm): an import question's upload, one row per upload (`import_parked_uploads`), and the handles its
-- page rendered (`import_parked_choices`), so a confirmation writes only the file and the destinations its page showed,
-- and at most once. `state`: open, written, refused, error, cancelled or expired; a terminal state commits with its
-- outcome. `sha256`/`bytes_len`: the parked bytes as uploaded (`<data>/import-parked/<token>`). `gen`: the question's
-- generation; only its handles resolve. `picks`: each explicit click (the venue, the account picked). A handle's
-- `dest_json`: the sealed answer it stands for and the route it was issued for. Rust-only and transient: 2.x never reads
-- either, a hand-back ignores them and a portable export copies no row.
CREATE TABLE import_parked_uploads (
  token TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL,
  sha256 TEXT NOT NULL,
  bytes_len INTEGER NOT NULL,
  format TEXT NOT NULL,
  filename TEXT,
  zone TEXT,
  gen INTEGER NOT NULL,
  state TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  picks TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL
);
CREATE TABLE import_parked_choices (
  handle TEXT PRIMARY KEY,
  token TEXT NOT NULL,
  gen INTEGER NOT NULL,
  kind TEXT NOT NULL,
  dest_json TEXT NOT NULL
);
CREATE INDEX index_import_parked_choices_on_token ON import_parked_choices (token, gen);
