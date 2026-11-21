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
-- One row per import run of a venue's catalogue in a scope (its category, '' for all), taken before its first unit and
-- completed (1) once every unit and its evidence were written; -1: a unit's evidence failed.
CREATE TABLE listing_observations (
  id INTEGER PRIMARY KEY,
  exchange_id INTEGER NOT NULL,
  scope TEXT NOT NULL,
  at TEXT NOT NULL,
  complete INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX index_listing_observations_on_scope ON listing_observations (exchange_id, scope, id);
-- Every ticker binding an import observed, as intervals: from first_seen to last_seen each run of the venue's scope found
-- `spelling` on `side` naming `asset_id` through `ticker_id`; observation_id is the last run that did. A run extends an
-- interval only from the scope's previous run, complete and recent (jobs::evidence), so a binding a run did not observe
-- starts a new one.
CREATE TABLE listing_evidence (
  id INTEGER PRIMARY KEY,
  exchange_id INTEGER NOT NULL,
  scope TEXT NOT NULL,
  side TEXT NOT NULL CHECK (side IN ('base', 'quote')),
  spelling TEXT NOT NULL,
  asset_id INTEGER NOT NULL,
  ticker_id INTEGER NOT NULL,
  first_seen TEXT NOT NULL,
  last_seen TEXT NOT NULL,
  observation_id INTEGER NOT NULL
);
CREATE INDEX index_listing_evidence_on_binding ON listing_evidence (exchange_id, side, spelling, scope, asset_id, ticker_id, last_seen DESC, id DESC);
