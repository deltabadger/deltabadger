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
-- completed (1) once every unit and its evidence were written; -1: a unit's evidence failed. A barrier (exchange_id NULL:
-- every venue) is a takeover, a handback or a catalogue snapshot's publish: no interval is extended across it.
CREATE TABLE listing_observations (
  id INTEGER PRIMARY KEY,
  exchange_id INTEGER,
  scope TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('import', 'barrier')),
  at TEXT NOT NULL,
  complete INTEGER NOT NULL DEFAULT 0
);
-- Every ticker binding an import observed, as intervals: from first_seen to last_seen each run of the venue's scope found
-- `spelling` on `side` naming `asset_id` through `ticker_id`; observation_id is the last run that did. A run extends an
-- interval only from its completed predecessor, so a binding a run did not observe starts a new one.
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
