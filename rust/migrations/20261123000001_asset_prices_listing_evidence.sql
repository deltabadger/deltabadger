-- Delisted coins (plan r10 §1.5, PR0): Rust-only, written beside 2.x's tables and read by nothing yet.
-- A coin's daily price keyed by the coin (its external id), not by the spelling a venue uses for it. Insert-only, as
-- historical_prices is; written after each provider answer's historical_prices write, in its own statement.
CREATE TABLE asset_prices (
  id INTEGER PRIMARY KEY,
  external_id TEXT NOT NULL,
  currency TEXT NOT NULL,
  date TEXT NOT NULL,
  price TEXT NOT NULL,
  UNIQUE (external_id, currency, date)
);
-- Every ticker binding an import observed, as intervals: from first_seen to last_seen each import of the venue's scope
-- (its category, '' for all) found `spelling` on `side` naming `asset_id` through `ticker_id`. A binding an import did
-- not observe ends its interval; one observed again later starts a new one. Written inside each import unit.
CREATE TABLE listing_evidence (
  id INTEGER PRIMARY KEY,
  exchange_id INTEGER NOT NULL,
  scope TEXT NOT NULL,
  side TEXT NOT NULL CHECK (side IN ('base', 'quote')),
  spelling TEXT NOT NULL,
  asset_id INTEGER NOT NULL,
  ticker_id INTEGER NOT NULL,
  first_seen TEXT NOT NULL,
  last_seen TEXT NOT NULL
);
CREATE INDEX index_listing_evidence_on_spelling ON listing_evidence (exchange_id, side, spelling);
-- An import unit at `at` whose evidence could not be written: no interval covers it.
CREATE TABLE listing_evidence_gap (
  exchange_id INTEGER NOT NULL,
  at TEXT NOT NULL
);
